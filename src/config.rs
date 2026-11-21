use crate::rime::Rime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};
pub fn open_rime(requested: Option<&str>) -> Result<Rime, Box<dyn std::error::Error>> {
    let settings = crate::settings::get()?;
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is missing")?);
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let source = env::var_os("GHOSTTY_CONSOLE_RIME_SOURCE")
        .map(PathBuf::from)
        .or_else(|| settings.rime_source_dir.clone())
        .unwrap_or_else(|| data.join("fcitx5/rime"));
    let user = settings
        .rime_user_dir
        .clone()
        .unwrap_or_else(|| data.join("ghostty-console/rime"));
    let prebuilt = source.join("build");
    if !prebuilt.is_dir() {
        return Err(format!("No compiled Rime data at {}", prebuilt.display()).into());
    }
    fs::create_dir_all(&user)?;
    // Only seed text configuration; never open/copy the live fcitx5 LevelDB user databases.
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("yaml" | "lua" | "txt")
            )
        {
            let target = user.join(entry.file_name());
            if !target.exists() {
                fs::copy(path, target)?;
            }
        }
    }
    for dir in ["lua", "opencc"] {
        copy_tree(&source.join(dir), &user.join(dir))?;
    }
    // Custom reverse-lookup translators resolve against user/build directly.
    let local_build = user.join("build");
    fs::create_dir_all(&local_build)?;
    for entry in fs::read_dir(&prebuilt)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .ends_with(".reverse.bin")
        {
            let target = local_build.join(entry.file_name());
            if !target.exists() {
                fs::copy(entry.path(), target)?;
            }
        }
    }
    let previous = fs::read_to_string(source.join("user.yaml")).unwrap_or_default();
    let schema = requested
        .or(settings.rime_schema.as_deref())
        .or_else(|| {
            previous.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("previously_selected_schema:")
                    .map(str::trim)
            })
        })
        .unwrap_or("luna_pinyin");
    Rime::open(&settings.rime_shared_dir, &user, &prebuilt, schema).map_err(Into::into)
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        } else if entry.file_type()?.is_file() && !target.join(entry.file_name()).exists() {
            fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}
