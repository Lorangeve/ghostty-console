#!/usr/bin/env fish
# Install root-owned service assets. The renderer always drops to the named user.
if test (id -u) -ne 0; or test (count $argv) -ne 1
    echo 'Usage: sudo fish --no-config scripts/install-tty6-login.fish USER' >&2
    exit 2
end
set -l target_user "$argv[1]"
set -l target_uid (id -u -- "$target_user"); or exit 1
if test "$target_uid" = 0; or not string match -qr '^[a-z_][a-z0-9_-]*[$]?$' -- "$target_user"
    echo 'Use an existing unprivileged renderer account.' >&2
    exit 1
end
set -l project_root (path resolve (path dirname (status filename))/..)
set -l user_home (getent passwd "$target_user" | cut -d: -f6)
set -l backup /var/lib/ghostty-console/prelogin-before-deploy
set -l binary "$project_root/target/release/ghostty-console"
if not test -x "$binary"
    echo 'Build the release binary first.' >&2
    exit 1
end
# Do not tear down an authenticated session when deploying from another VT.
if test -s /run/ghostty-console/tty6-session
    echo 'Log out of tty6 before installing the login frontend.' >&2
    exit 1
end
for session in (loginctl list-sessions --no-legend | string trim | string split -n ' ' -f1)
    if test (loginctl show-session "$session" -p VTNr --value) = 6; and test (loginctl show-session "$session" -p State --value) != closing
        echo 'Log out of tty6 before installing the login frontend.' >&2
        exit 1
    end
end
install -d -m 700 "$backup"; or exit 1
if not test -e "$backup/complete"
    for item in autovt@tty6.service getty.target.wants/getty@tty6.service getty.target.wants/ghostty-console@tty6.service
        set -l original "/etc/systemd/system/$item"
        if test -e "$original"; or test -L "$original"
            install -d -m 700 (path dirname "$backup/$item"); or exit 1
            cp -a "$original" "$backup/$item"; or exit 1
        end
    end
    if test -f "$user_home/.config/fish/conf.d/ghostty-console-tty6.fish"
        cp -a "$user_home/.config/fish/conf.d/ghostty-console-tty6.fish" "$backup/login-hook.fish"; or exit 1
    end
    touch "$backup/complete"; or exit 1
end
# A fixed, non-login account owns the greeter. Its home and font config are
# root-owned: no real user's scripts, fonts or cache are read before PAM login.
if not getent passwd ghostty-console >/dev/null
    useradd --system --user-group --home-dir /var/lib/ghostty-console/greeter --shell /usr/bin/nologin ghostty-console; or exit 1
end
if test (id -u ghostty-console) = 0
    echo 'Invalid greeter account' >&2
    exit 1
end
install -d -o root -g root -m 755 /var/lib/ghostty-console/greeter; or exit 1
install -d -m 755 /usr/local/bin /usr/local/libexec /etc/ghostty-console /etc/systemd/system/getty.target.wants; or exit 1
install -m 755 "$project_root/scripts/ghostty-console-login-shell" /usr/local/libexec/ghostty-console-login-shell; or exit 1
set -l staged (mktemp /usr/local/bin/.ghostty-console.XXXXXX); or exit 1
install -m 755 "$binary" "$staged"; and mv -T "$staged" /usr/local/bin/ghostty-console; or exit 1
if not test -e /etc/ghostty-console/config
    install -m 644 "$project_root/ghostty-console.conf" /etc/ghostty-console/config; or exit 1
end
/usr/local/bin/ghostty-console --config-file /etc/ghostty-console/config +validate-config; or exit 1
printf 'CON_RENDER_USER=%s\n' "$target_user" > /etc/ghostty-console/tty6.env; or exit 1
chmod 644 /etc/ghostty-console/tty6.env; or exit 1
install -m 644 "$project_root/systemd/ghostty-console@.service" /etc/systemd/system/ghostty-console@.service; or exit 1
systemd-analyze verify /etc/systemd/system/ghostty-console@.service; or exit 1
ln -sfn /etc/systemd/system/ghostty-console@.service /etc/systemd/system/autovt@tty6.service; or exit 1
ln -sfn /etc/systemd/system/ghostty-console@.service /etc/systemd/system/getty.target.wants/ghostty-console@tty6.service; or exit 1
rm -f /etc/systemd/system/getty.target.wants/getty@tty6.service; or exit 1
rm -f "$user_home/.config/fish/conf.d/ghostty-console-tty6.fish"; or exit 1
systemctl daemon-reload; or exit 1
systemctl reset-failed ghostty-console@tty6.service; or exit 1
systemctl restart ghostty-console@tty6.service; or exit 1
echo 'tty6 now shows login directly inside con; the old fish autostart hook was removed.'
