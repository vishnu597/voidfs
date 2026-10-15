-- SPDX-License-Identifier: Apache-2.0
-- One Finder operation per run, with the same Finder `duplicate` verbs as
-- apps/macos/scripts/app-matrix-cocoa.applescript's finder-copy, so that each module trace
-- window belongs to one operation.
--   osascript finder.applescript copy <file> <folder>   copy a file into a folder
--   osascript finder.applescript duplicate <file>       Finder Duplicate, in place
-- Prints the POSIX path of the new file.
on run argv
    set operation to item 1 of argv
    set sourceFile to (POSIX file (item 2 of argv)) as alias
    if operation is "copy" then
        set targetFolder to (POSIX file (item 3 of argv)) as alias
        tell application "Finder" to set made to duplicate sourceFile to targetFolder
    else if operation is "duplicate" then
        tell application "Finder" to set made to duplicate sourceFile
    else
        error "unknown operation " & operation
    end if
    return POSIX path of (made as alias)
end run
