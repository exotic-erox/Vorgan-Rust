# Vorgan launcher for fish. Copy to ~/.config/fish/functions/vg.fish
# Quit with q to land your shell in the folder you ended in, Q to stay where you were.
function vg --description 'Vorgan file manager'
    set -l tmp (mktemp -t vorgan.XXXXXX)
    command vorgan --cwd-file=$tmp $argv
    if test -s $tmp
        set -l dir (cat -- $tmp)
        if test -d "$dir"; and test "$dir" != "$PWD"
            cd -- $dir
        end
    end
    command rm -f -- $tmp
end
