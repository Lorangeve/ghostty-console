use std::process::Command;

/// Requires this machine's installed Rime schemas; exercises the actual C ABI and dictionaries.
#[test]
#[ignore = "requires local fcitx5 Rime data with py and wubi98_ci schemas"]
fn existing_schemas_commit_chinese() {
    let original_data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share")
        });
    let source = std::env::var_os("GHOSTTY_CONSOLE_RIME_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| original_data.join("fcitx5/rime"));
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dirname = format!("ghostty-console-rime-test-{}-{nonce}", std::process::id());
    let scratch = Scratch(std::env::temp_dir().join(dirname));
    std::fs::create_dir(&scratch.0).unwrap();
    for (schema, keys) in [("py", "nihao"), ("wubi98_ci", "wqvb")] {
        let output = Command::new(env!("CARGO_BIN_EXE_ghostty-console"))
            .args(["rime-probe", schema, keys])
            .env("XDG_DATA_HOME", &scratch.0)
            .env("GHOSTTY_CONSOLE_RIME_SOURCE", &source)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stderr.is_empty(), "native engine diagnostics: {stderr}");
        assert!(stdout.contains("你好"), "{stdout}");
        assert!(stdout.contains("commit=\"你好\""), "{stdout}");
        assert!(stdout.contains("handled=true preedit="), "{stdout}");
    }
}

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
