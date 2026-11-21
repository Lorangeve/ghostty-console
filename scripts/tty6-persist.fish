#!/usr/bin/env fish
# Persist the already-tested native getty setup, touching only tty6.
if test (id -u) -ne 0
    echo 'Run: sudo fish --no-config ~/mycoding/ghostty-console/scripts/tty6-persist.fish' >&2
    exit 1
end
set -l systemd_dir /etc/systemd/system
set -l backup /var/lib/ghostty-console/tty6-before-deploy
set -l paths autovt@tty6.service kmsconvt@tty6.service getty.target.wants/getty@tty6.service getty.target.wants/kmsconvt@tty6.service
# Preserve original instance configuration once; subsequent deploys are idempotent.
if not test -e "$backup/complete"
    install -d -m 700 "$backup"; or exit 1
    for item in $paths
        if test -e "$systemd_dir/$item"; or test -L "$systemd_dir/$item"
            install -d -m 700 (path dirname "$backup/$item"); or exit 1
            cp -a -- "$systemd_dir/$item" "$backup/$item"; or exit 1
        end
    end
    touch "$backup/complete"; or exit 1
end
# Do not interrupt a logged-in kmscon instance.
if systemctl is-active --quiet kmsconvt@tty6.service
    echo 'tty6 still uses kmscon. Log out and run the temporary prepare script first.' >&2
    exit 1
end
ln -sfn /dev/null "$systemd_dir/kmsconvt@tty6.service"; or exit 1
ln -sfn /usr/lib/systemd/system/getty@.service "$systemd_dir/autovt@tty6.service"; or exit 1
install -d -m 755 "$systemd_dir/getty.target.wants"; or exit 1
ln -sfn /usr/lib/systemd/system/getty@.service "$systemd_dir/getty.target.wants/getty@tty6.service"; or exit 1
rm -f "$systemd_dir/getty.target.wants/kmsconvt@tty6.service"; or exit 1
# The current temporary masks would otherwise override the persistent autovt link.
rm -f /run/systemd/system/autovt@tty6.service /run/systemd/system/kmsconvt@tty6.service; or exit 1
systemctl daemon-reload; or exit 1
systemctl start getty@tty6.service; or exit 1
echo "tty6 now persistently uses native getty. Original links: $backup"
echo 'Switch to tty6 and log in normally; fish starts con. Ctrl+Alt+Esc returns to fish.'
