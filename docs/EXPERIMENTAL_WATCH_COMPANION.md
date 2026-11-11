# Experimental watchOS companion and App Group support

> This is documentation for an **unofficial fork**, not a claim of support by the upstream project.

The actual code for the working Windows Apple Watch companion build is in:

- **Exact backend commit:** [`323e5341b6f02a182e2c913a09ca43f553322758`](https://github.com/AlsoSama/isideload/tree/323e5341b6f02a182e2c913a09ca43f553322758)
- **Branch:** [`fix/watch-companion-appgroup-27.3b1`](https://github.com/AlsoSama/isideload/tree/fix/watch-companion-appgroup-27.3b1)
- **Exact paired iLoader commit:** [`bef7e279769022ded6618520c1fbaf1f6388c084`](https://github.com/AlsoSama/iloader/tree/bef7e279769022ded6618520c1fbaf1f6388c084)
- **Successful Windows installer build:** [Actions run 43020614046](https://github.com/AlsoSama/iloader/actions/runs/43020614046)
- **User instructions and limitations:** [Windows companion guide](https://github.com/AlsoSama/iloader/blob/main/docs/WINDOWS_WATCH_COMPANION.md)

## Implementation areas

- `isideload/src/sideload/bundle.rs`: iOS + watchOS bundle metadata and companion identifier reconciliation.
- `isideload/src/sideload/watch_install.rs`: experimental installation path for Apple Watch companion applications.
- `isideload/src/sideload/sideloader.rs`: registers companion App IDs, performs signing/provisioning, and checks required watchOS App Group entitlements before installation.
- `isideload/src/dev/app_ids.rs` and `isideload/src/dev/app_groups.rs`: Apple developer service watchOS capability and App Group assignment support.
- `isideload/src/anisette/remote_v3/mod.rs` and `isideload/src/auth/grandslam.rs`: authentication client interoperability adjustments associated with HTTP 503 and 429 failures.

The backend checks that the watch provisioning profile includes the expected App Group entitlement, avoiding a previously silent omission.

## Privacy

This documentation intentionally contains **no personal Apple Account email, password, verification code, signing credential, device identifier, provisioning profile, or troubleshooting log**. The repository does not publish an end-user session snapshot. Do not add real login material to a bug report or commit; redact logs before sharing. Runtime Apple authentication is still sensitive: review the code, operating-system credential storage, and the chosen Anisette provider.

## Compatibility and responsibility

- Observed to work only in a narrow experimental setup. Other Apple Watch models, system releases, Apple developer accounts, and provider behavior are not generally verified.
- This code does not override Apple's hardware, developer-team, entitlements, services or device eligibility restrictions.
- Retain original app-specific backups before using any MobileGestalt modification tool.
- The `main` branch is **not** a copy of this tested change set; use the exact linked commit to reproduce the installer.
- Respect MIT copyright/license notices and avoid implying upstream endorsement.

## 致中文用户

这是**非官方实验性修改**，并非上游正式版本。已完成的 Watch companion 支持、App Group 授权及认证相关修复在上述提交中；Windows 安装包请从 iLoader 的 GitHub Actions 成功构建记录获取。**不要公开 Apple ID、密码、签名证书、描述文件、设备 UDID 或未经脱敏的日志。**
