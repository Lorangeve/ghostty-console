#!/usr/bin/env fish
# Run from your existing desktop. Uses runtime masks only; no persistent enablement changes.
if test (count $argv) -ne 1; or not contains -- $argv[1] prepare restore
    echo 'Usage: fish scripts/tty6.fish prepare|restore' >&2
    exit 2
end
set -l sessions (loginctl list-sessions --no-legend | string trim | string split -n ' ' -f1)
for session in $sessions
    if test (loginctl show-session "$session" --property=VTNr --value) = 6
        echo 'tty6 has a logged-in session. Log out of it before changing the console service.' >&2
        exit 1
    end
end
switch $argv[1]
    case prepare
        # logind starts autovt@tty6 when switching VTs, even after kmscon was stopped.
        sudo systemctl mask --runtime kmsconvt@tty6.service autovt@tty6.service; or exit 1
        if not sudo systemctl stop kmsconvt@tty6.service; or not sudo systemctl start getty@tty6.service
            sudo systemctl unmask --runtime kmsconvt@tty6.service autovt@tty6.service
            sudo systemctl start kmsconvt@tty6.service
            exit 1
        end
        echo 'tty6 now uses native getty. Press Ctrl+Alt+F6, log in, then run:'
        echo '  cd ~/mycoding/ghostty-console'
        echo '  cargo run --release -- con --timeout 120'
        echo 'After testing: exit the tty6 login shell, return to tty1, and run this script with restore.'
    case restore
        sudo systemctl unmask --runtime kmsconvt@tty6.service autovt@tty6.service; or exit 1
        sudo systemctl stop getty@tty6.service; or exit 1
        sudo systemctl start kmsconvt@tty6.service; or exit 1
        echo 'kmscon restored on tty6.'
end
