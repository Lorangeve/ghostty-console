#!/usr/bin/env fish
# User install by default. Build as the invoking user; elevate only system file operations.
argparse 'h/help' 'prefix=' 'config-dir=' 'no-build' 'system' -- $argv; or exit 2
if set -q _flag_help
    echo 'Usage: fish install.fish [--system] [--prefix DIR] [--config-dir DIR] [--no-build]'
    echo 'Default: ~/.local/bin/ghostty-console and $XDG_CONFIG_HOME/ghostty-console/config.'
    echo '--system: /usr/local/bin/ghostty-console and /etc/ghostty-console/config.'
    echo 'Existing configuration is preserved. No service or shell PATH settings are changed.'
    exit 0
end
if test (count $argv) -ne 0
    echo "Unexpected argument: $argv[1]" >&2
    exit 2
end
set -l project_root (path dirname (status filename) | path resolve)
set -l prefix "$HOME/.local"
set -l config_home "$HOME/.config"
if set -q XDG_CONFIG_HOME; and test -n "$XDG_CONFIG_HOME"
    set config_home "$XDG_CONFIG_HOME"
end
set -l config_dir "$config_home/ghostty-console"
set -l gc_install_elevate false
if set -q _flag_system
    set prefix /usr/local
    set config_dir /etc/ghostty-console
    if test (id -u) -ne 0
        set gc_install_elevate true
    end
end
if set -q _flag_prefix
    set prefix "$_flag_prefix"
end
if set -q _flag_config_dir
    set config_dir "$_flag_config_dir"
end
if test -z "$prefix"; or test -z "$config_dir"
    echo '--prefix and --config-dir must not be empty' >&2
    exit 2
end
set prefix (path resolve -- "$prefix")
set config_dir (path resolve -- "$config_dir")
function __gc_install_run --inherit-variable gc_install_elevate
    if test "$gc_install_elevate" = true
        command sudo -- $argv
    else
        command $argv
    end
end
if not set -q _flag_no_build
    for program in cargo pkg-config cc fc-match fc-list
        if not type -q "$program"
            echo "Missing build/runtime tool: $program" >&2
            exit 1
        end
    end
    for library in rime libghostty-vt libseat xkbcommon freetype2 libdrm libsystemd
        if not pkg-config --exists "$library"
            echo "Missing native dependency: $library (install its development headers/pkg-config metadata)" >&2
            exit 1
        end
    end
    cargo build --release --locked --target-dir "$project_root/target" --manifest-path "$project_root/Cargo.toml"; or exit 1
end
set -l binary "$project_root/target/release/ghostty-console"
if not test -x "$binary"
    echo "Built binary not found: $binary" >&2
    exit 1
end
# Validate the bundled configuration before changing the destination.
"$binary" --config-file "$project_root/ghostty-console.conf" +validate-config; or exit 1
for directory in "$prefix/bin" "$config_dir"
    if not test -d "$directory"
        __gc_install_run install -d -m 755 -- "$directory"; or exit 1
    end
end
set -l staged (__gc_install_run mktemp "$prefix/bin/.ghostty-console.XXXXXX"); or exit 1
if not __gc_install_run install -m 755 -- "$binary" "$staged"; or not __gc_install_run mv -T -- "$staged" "$prefix/bin/ghostty-console"
    __gc_install_run rm -- "$staged"
    exit 1
end
set -l config_path "$config_dir/config"
if test -e "$config_path"; or test -L "$config_path"
    echo "Preserved existing config: $config_path"
else
    set -l staged_config (__gc_install_run mktemp "$config_dir/.config.XXXXXX"); or exit 1
    if not __gc_install_run install -m 644 -- "$project_root/ghostty-console.conf" "$staged_config"
        __gc_install_run rm -- "$staged_config"
        exit 1
    end
    # Atomic create-only link: a concurrently created user configuration is never overwritten.
    if not __gc_install_run ln -- "$staged_config" "$config_path"
        __gc_install_run rm -- "$staged_config"
        if not test -e "$config_path"; and not test -L "$config_path"
            exit 1
        end
        echo "Preserved concurrently created config: $config_path"
    else
        __gc_install_run rm -- "$staged_config"
        echo "Installed config: $config_path"
    end
end
echo "Installed binary: $prefix/bin/ghostty-console"
echo "Run from a native VT: $prefix/bin/ghostty-console con"
if not contains -- "$prefix/bin" $PATH
    echo "To add the command to fish's PATH: fish_add_path '$prefix/bin'"
end
