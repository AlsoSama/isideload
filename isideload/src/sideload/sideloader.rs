use crate::{
    dev::{
        app_groups::AppGroupsApi,
        app_ids::{AppIdsApi, Profile},
        developer_session::DeveloperSession,
        device_type::DeveloperDeviceType,
        devices::DevicesApi,
        teams::{DeveloperTeam, TeamsApi},
    },
    sideload::{
        TeamSelection,
        application::{Application, SpecialApp},
        builder::MaxCertsBehavior,
        bundle::Bundle,
        cert_identity::CertificateIdentity,
        sign,
    },
    util::{device::IdeviceInfo, storage::SideloadingStorage},
};

use std::path::PathBuf;

use apple_codesign::ProvisioningProfile;
use idevice::{
    IdeviceService,
    provider::IdeviceProvider,
    services::companion_proxy::CompanionProxy,
};
use plist::Dictionary;
use rootcause::{option_ext::OptionExt, prelude::*};
use tracing::info;

pub struct Sideloader {
    team_selection: TeamSelection,
    storage: Box<dyn SideloadingStorage>,
    dev_session: DeveloperSession,
    machine_name: String,
    apple_email: String,
    max_certs_behavior: MaxCertsBehavior,
    //extensions_behavior: ExtensionsBehavior,
    delete_app_after_install: bool,
    team: Option<DeveloperTeam>,
}

impl Sideloader {
    /// Construct a new `Sideloader` instance with the provided configuration
    ///
    /// See [`crate::sideload::SideloaderBuilder`] for more details and a more convenient way to construct a `Sideloader`.
    pub fn new(
        dev_session: DeveloperSession,
        apple_email: String,
        team_selection: TeamSelection,
        max_certs_behavior: MaxCertsBehavior,
        machine_name: String,
        storage: Box<dyn SideloadingStorage>,
        //extensions_behavior: ExtensionsBehavior,
        delete_app_after_install: bool,
    ) -> Self {
        Sideloader {
            team_selection,
            storage,
            dev_session,
            machine_name,
            apple_email,
            max_certs_behavior,
            //extensions_behavior,
            delete_app_after_install,
            team: None,
        }
    }

    /// Sign the app at the provided path and return the path to the signed app bundle (in a temp dir). To sign and install, see [`Self::install_app`].
    pub async fn sign_app<F, Fut>(
        &mut self,
        app_path: PathBuf,
        team: Option<DeveloperTeam>,
        // this will be replaced with proper entitlement handling later
        increased_memory_limit: bool,
        progress_callback: Option<F>,
    ) -> Result<(PathBuf, Option<SpecialApp>), Report>
    where
        F: Fn(f32) -> Fut,
        Fut: Future<Output = ()>,
    {
        let team = match team {
            Some(t) => t,
            None => self.get_team().await?,
        };
        let cert_identity = CertificateIdentity::retrieve(
            &self.machine_name,
            &self.apple_email,
            &mut self.dev_session,
            &team,
            self.storage.as_ref(),
            &self.max_certs_behavior,
        )
        .await
        .context("Failed to retrieve certificate identity")?;

        if let Some(callback) = &progress_callback {
            callback(0.1).await;
        }

        let mut app = Application::new(app_path)?;
        let special = app.get_special_app();

        let main_bundle_id = app.main_bundle_id()?;
        let main_app_name = app.main_app_name()?;
        let main_app_id_str = format!("{}.{}", main_bundle_id, team.team_id);
        app.update_bundle_id(&main_bundle_id, &main_app_id_str)?;
        let watch_bundle_ids = app.watch_bundle_ids();

        // Unsigned CI-built apps do not carry a signed entitlements blob. HealthKit's
        // required Info.plist privacy keys do survive packaging, so use those keys to
        // identify every app-ID-bearing bundle that needs the HealthKit App ID service.
        let healthkit_bundle_ids = app
            .bundle
            .collect_app_id_bundles()
            .into_iter()
            .filter(|bundle| {
                bundle.app_info.contains_key("NSHealthShareUsageDescription")
                    || bundle.app_info.contains_key("NSHealthUpdateUsageDescription")
            })
            .filter_map(|bundle| bundle.bundle_identifier().map(str::to_string))
            .collect::<std::collections::HashSet<_>>();

        let mut app_ids = app
            .register_app_ids(
                /*&self.extensions_behavior, */ &mut self.dev_session,
                &team,
            )
            .await?;
        let main_app_id = match app_ids
            .iter()
            .find(|app_id| app_id.identifier == main_app_id_str)
        {
            Some(id) => id,
            None => {
                bail!(
                    "Main app ID {} not found in registered app IDs",
                    main_app_id_str
                );
            }
        }
        .clone();

        let group_identifier = format!(
            "group.{}",
            if Some(SpecialApp::SideStoreLc) == special {
                format!("com.SideStore.SideStore.{}", team.team_id)
            } else {
                main_app_id_str.clone()
            }
        );

        let app_group = self
            .dev_session
            .ensure_app_group(&team, &main_app_name, &group_identifier, None)
            .await?;

        for app_id in app_ids.iter_mut() {
            if healthkit_bundle_ids.contains(&app_id.identifier) {
                let device_type = if watch_bundle_ids.contains(&app_id.identifier) {
                    Some(DeveloperDeviceType::Watchos)
                } else {
                    Some(DeveloperDeviceType::Ios)
                };

                app_id
                    .ensure_healthkit_feature(&mut self.dev_session, &team, device_type)
                    .await
                    .context(format!(
                        "Failed to configure HealthKit capability for {}",
                        app_id.identifier
                    ))?;
            }

            // Watch applications need an authorized application group in their
            // watchOS provisioning profile (e.g. HermesEnabler's MG access).
            // Preserve the shared host app group, but configure it using the
            // watchOS platform marker for Watch App IDs.
            let is_watch_app = watch_bundle_ids.contains(&app_id.identifier);
            let group_platform = is_watch_app.then_some(DeveloperDeviceType::Watchos);

            app_id
                .ensure_group_feature(
                    &mut self.dev_session,
                    &team,
                    group_platform.clone(),
                )
                .await
                .context(format!(
                    "Failed to enable App Groups for {}",
                    app_id.identifier
                ))?;

            self.dev_session
                .assign_app_group(&team, &app_group, app_id, group_platform)
                .await
                .context(format!(
                    "Failed to assign App Group {} to {}",
                    group_identifier, app_id.identifier
                ))?;

            if !is_watch_app && increased_memory_limit {
                self.dev_session
                    .add_increased_memory_limit(&team, app_id)
                    .await?;
            }
        }

        if let Some(callback) = &progress_callback {
            callback(0.15).await;
        }

        info!("App IDs configured");

        app.apply_special_app_behavior(&special, &group_identifier, &cert_identity)
            .await
            .context("Failed to modify app bundle")?;

        let main_provisioning_profile = self
            .dev_session
            .download_team_provisioning_profile(&team, &main_app_id, None)
            .await?;

        let mut provisioning_profiles: Vec<(String, Profile, Dictionary)> = Vec::new();

        for id in app_ids
            .into_iter()
            .filter(|id| id.identifier != main_app_id.identifier)
        {
            let bundle_id = id.identifier.clone();

            let profile_device_type = if watch_bundle_ids.contains(&bundle_id) {
                Some(DeveloperDeviceType::Watchos)
            } else {
                None
            };

            let profile = self
                .dev_session
                .download_team_provisioning_profile(&team, &id, profile_device_type)
                .await
                .context(format!(
                    "Failed to download provisioning profile for {}",
                    bundle_id
                ))?;

            let parsed_profile = ProvisioningProfile::parse(profile.encoded_profile.as_ref())?;
            let entitlements = parsed_profile.entitlements().clone();

            // Never silently sign a Watch app without the group we requested.
            // The installed app may launch but its MG backup will fail.
            if watch_bundle_ids.contains(&bundle_id) {
                let has_group = entitlements
                    .get("com.apple.security.application-groups")
                    .and_then(plist::Value::as_array)
                    .is_some_and(|groups| {
                        groups.iter().any(|group| {
                            group.as_string() == Some(group_identifier.as_str())
                        })
                    });
                if !has_group {
                    bail!(
                        "watchOS provisioning profile for {} lacks App Group {}.                          The Apple developer team may not authorize this capability.",
                        bundle_id, group_identifier
                    );
                }
            }

            provisioning_profiles.push((bundle_id, profile, entitlements));
        }

        if let Some(callback) = &progress_callback {
            callback(0.2).await;
        }

        info!("Acquired provisioning profile");

        // Persist all rewritten Info.plists, including embedded watchOS apps and their extensions,
        // before apple-codesign scans and signs the bundle tree.
        app.bundle.write_info_recursive()?;

        if let Some(callback) = &progress_callback {
            callback(0.3).await;
        }

        sign::sign(
            &mut app,
            &cert_identity,
            &main_provisioning_profile,
            &provisioning_profiles,
            &special,
            &team,
            progress_callback,
        )
        .await
        .context("Failed to sign app")?;

        info!("App signed!");

        Ok((app.bundle.bundle_dir.clone(), special))
    }

    #[cfg(feature = "install")]
    /// Sign and install an app to a device.
    pub async fn install_app<F, Fut>(
        &mut self,
        device_provider: &impl IdeviceProvider,
        app_path: PathBuf,
        // this is gross but will be replaced with proper entitlement handling later
        increased_memory_limit: bool,
        progress_callback: Option<F>,
    ) -> Result<Option<SpecialApp>, Report>
    where
        F: Fn(f32) -> Fut,
        Fut: Future<Output = ()>,
    {
        let device_info = IdeviceInfo::from_device(device_provider).await?;

        let team = self.get_team().await?;
        self.dev_session
            .ensure_device_registered(&team, &device_info.name, &device_info.udid, None)
            .await?;

        // Only iPhone apps containing a watchOS companion require the paired-Watch
        // registry. In particular, iPadOS responds to get_device_registry() with
        // UnsupportedDevice even when the IPA is a perfectly ordinary iPad app.
        // Inspect the same bundle layout used by sign_app before requesting the
        // iPhone-only companion_proxy service.
        let includes_watch_companion = !Application::new(app_path.clone())?
            .bundle
            .watch_apps()
            .is_empty();

        if includes_watch_companion {
            // Register paired Watches before requesting watchOS provisioning profiles.
            let mut provisioning_companion_proxy = CompanionProxy::connect(device_provider)
                .await
                .context("A Watch companion IPA requires a paired iPhone with companion_proxy support")?;

            let paired_watches = provisioning_companion_proxy
                .get_device_registry()
                .await
                .context("Cannot list paired Apple Watches; select the paired iPhone for Watch apps")?;

            for watch_udid in paired_watches {
                let watch_name = provisioning_companion_proxy
                    .get_value(&watch_udid, "DeviceName")
                    .await
                    .ok()
                    .and_then(|value| value.as_string().map(str::to_string))
                    .unwrap_or_else(|| "Apple Watch".to_string());

                self.dev_session
                    .ensure_device_registered(
                        &team,
                        &watch_name,
                        &watch_udid,
                        Some(DeveloperDeviceType::Watchos),
                    )
                    .await
                    .context("Failed to register paired Apple Watch as a development device")?;
            }

            // The iPhone companion proxy may be invalidated by an installation.
            // Reconnect for post-install Watch transfer rather than reusing it.
            drop(provisioning_companion_proxy);
        }

        let (signed_app_path, special_app) = self
            .sign_app(
                app_path,
                Some(team),
                increased_memory_limit,
                progress_callback,
            )
            .await?;

        info!("Transferring App...");

        crate::sideload::install::install_app(device_provider, &signed_app_path, |progress| {
            info!("Installing: {}%", progress);
        })
        .await
        .context("Failed to install app on device")?;

        // The iPhone install may leave the embedded Watch app as a process-scoped placeholder.
        // Re-open the signed bundle before cleanup and install each Watch app directly through
        // the paired Watch's streaming_zip_conduit service. This is the path physically validated
        // on watchOS after the signed Watch bundle itself was proven valid.
        let signed_bundle = Bundle::new(signed_app_path.clone())?;
        let watch_apps = signed_bundle.watch_apps().to_vec();

        if !watch_apps.is_empty() {
            info!("Installing Apple Watch companion app directly with fresh companion proxy connections...");
            crate::sideload::watch_install::install_watch_apps(
                device_provider,
                &watch_apps,
                &self.machine_name,
                |progress| {
                    info!("Installing Apple Watch app: {}%", progress);
                },
            )
            .await
            .context("Failed to install Apple Watch companion app")?;
        }

        if self.delete_app_after_install
            && let Err(e) = isideload_vfs::fs::remove_dir_all(signed_app_path)
        {
            tracing::warn!("Failed to remove temporary signed app file: {}", e);
        }

        Ok(special_app)
    }

    /// Get the developer team according to the configured team selection behavior.
    pub async fn get_team(&mut self) -> Result<DeveloperTeam, Report> {
        if let Some(team) = &self.team {
            return Ok(team.clone());
        }
        let teams = self.dev_session.list_teams().await?;
        let team = match teams.len() {
            0 => {
                bail!("No developer teams available")
            }
            1 => teams.into_iter().next().ok_or_report()?,
            _ => {
                info!(
                    "Multiple developer teams found, {} as per configuration",
                    self.team_selection
                );
                match &self.team_selection {
                    TeamSelection::First => teams.into_iter().next().ok_or_report()?,
                    TeamSelection::PromptOnce(prompt_fn)
                    | TeamSelection::PromptAlways(prompt_fn) => {
                        let selection =
                            prompt_fn(&teams).ok_or_else(|| report!("No team selected"))?;
                        teams
                            .into_iter()
                            .find(|t| t.team_id == selection)
                            .ok_or_else(|| report!("No team found with ID {}", selection))?
                    }
                }
            }
        };
        if !matches!(&self.team_selection, TeamSelection::PromptAlways(_)) {
            self.team = Some(team.clone());
        }
        Ok(team)
    }

    pub fn get_dev_session(&mut self) -> &mut DeveloperSession {
        &mut self.dev_session
    }

    pub fn get_email(&self) -> &str {
        &self.apple_email
    }
}
