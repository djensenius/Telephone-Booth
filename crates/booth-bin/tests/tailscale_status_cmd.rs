//! CLI tests for `telephone-booth tailscale-status`.

#![cfg(unix)]

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn tailscale_status_pretty_prints_mock_json() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tailscale-status")
        .join(std::process::id().to_string());
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;

    let tailscale = root.join("tailscale");
    fs::write(
        &tailscale,
        r#"#!/bin/sh
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
cat <<'JSON'
{"Self":{"DNSName":"phone-booth.tailnet.ts.net."},"Health":["ok"]}
JSON
exit 0
fi
if [ "$1" = "serve" ] && [ "$2" = "status" ] && [ "$3" = "--json" ]; then
cat <<'JSON'
{"TCP":{"443":{"HTTPS":true,"Path":"/","Proxy":"http://127.0.0.1:8080"}}}
JSON
exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
    )?;
    let mut permissions = fs::metadata(&tailscale)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tailscale, permissions)?;

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = std::env::join_paths(
        std::iter::once(root.clone()).chain(std::env::split_paths(&old_path)),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_telephone-booth"))
        .arg("tailscale-status")
        .env("PATH", new_path)
        .output()?;

    fs::remove_dir_all(&root)?;

    assert!(
        output.status.success(),
        "tailscale-status failed: {output:?}"
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("magicdnsname: phone-booth.tailnet.ts.net"));
    assert!(stdout.contains("url: https://phone-booth.tailnet.ts.net"));
    assert!(stdout.contains("health:"));
    assert!(stdout.contains("serve_config:"));
    assert!(stdout.contains("127.0.0.1:8080"));
    Ok(())
}
