use std::process::{Command, Stdio};
#[test]
fn refuses_to_take_over_devices_from_non_vt_stdin() {
    let result = Command::new(env!("CARGO_BIN_EXE_ghostty-console"))
        .args(["con"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("native /dev/ttyN"));
}
#[test]
fn login_modes_require_privileged_service_startup() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    for action in ["login-service", "login-worker", "login-cleanup"] {
        let result = Command::new(env!("CARGO_BIN_EXE_ghostty-console"))
            .args([action, "lorangeve"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("root"));
    }
}

#[test]
fn cleanup_does_not_parse_renderer_configuration() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let result = Command::new(env!("CARGO_BIN_EXE_ghostty-console"))
        .args([
            "--config-file",
            "/nonexistent/con-renderer-config",
            "login-cleanup",
        ])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("requires root"));
    assert!(!error.contains("No such file"));
}
