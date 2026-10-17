#!/bin/bash
# Closes the Finder windows showing a folder on the test mount, and only those: close-windows.sh <mountpoint>.
/usr/bin/osascript - "$1" <<'OSA'
on run argv
  set m to item 1 of argv
  set n to 0
  tell application "Finder"
    repeat with w in (every Finder window)
      try
        set p to POSIX path of (target of w as alias)
        if p starts with m then
          close w
          set n to n + 1
        end if
      end try
    end repeat
  end tell
  return "closed " & n
end run
OSA
