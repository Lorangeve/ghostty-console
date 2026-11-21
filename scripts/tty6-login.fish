# Installed as ~/.config/fish/conf.d/ghostty-console-tty6.fish.
# Start once, after the login shell has finished loading its configuration.
if status is-interactive; and status is-login
    function __ghostty_console_tty6_login --on-event fish_prompt
        functions --erase __ghostty_console_tty6_login
        if set -q GHOSTTY_CONSOLE_NO_AUTOSTART
            return
        end
        if test (command tty 2>/dev/null) != /dev/tty6
            return
        end
        set -l binary "$HOME/.local/bin/ghostty-console"
        if not test -x "$binary"
            return
        end
        # A normal return leaves this getty login's fish available for recovery.
        command "$binary" con --timeout 0
    end
end
