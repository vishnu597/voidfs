-- SPDX-License-Identifier: Apache-2.0
-- osascript app-matrix-cocoa.applescript finder-copy <source-dir> <mount-dir> <export-dir>
-- osascript app-matrix-cocoa.applescript finder-trash <copied-dir>
-- osascript app-matrix-cocoa.applescript textedit <fresh-file>
on run argv
    set operation to item 1 of argv
    if operation is "finder-copy" then
        set sourceFolder to POSIX file (item 2 of argv)
        set mountFolder to POSIX file (item 3 of argv)
        set exportFolder to POSIX file (item 4 of argv)
        tell application "Finder"
            set copiedFolder to duplicate sourceFolder to mountFolder
            set copiedFile to file "xattr.bin" of copiedFolder
            set extraCopy to duplicate copiedFile
            set name of extraCopy to "duplicate.bin"
            set name of copiedFile to "renamed.bin"
            set copiedFile to file "renamed.bin" of copiedFolder
            set label index of copiedFile to 3
            set copiedOut to duplicate copiedFile to exportFolder
            open information window of copiedFile
            set detail to {name of copiedFile, size of copiedFile, label index of copiedFile, POSIX path of (copiedFolder as alias), POSIX path of (copiedOut as alias)}
            close information window of copiedFile
            return detail
        end tell
    else if operation is "finder-trash" then
        tell application "Finder"
            delete file "duplicate.bin" of folder (POSIX file (item 2 of argv))
        end tell
        return "Move to Trash requested for scratch duplicate.bin"
    else if operation is "textedit" then
        set targetFile to POSIX file (item 2 of argv)
        tell application "TextEdit"
            set doc to open targetFile
            set text of doc to "application matrix initial draft — 名前\n"
            save doc
            set text of doc to "application matrix edited draft — 名前\n"
            save doc
            close doc saving no
            set reopened to open targetFile
            set resultText to text of reopened
            close reopened saving no
            return resultText
        end tell
    else
        error "unknown application matrix operation"
    end if
end run
