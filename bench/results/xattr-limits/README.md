# Extended attribute sizes on APFS

15 October 2026, macOS 27.0.1 (26A434), Apple M5 Pro, the Data volume (APFS). Evidence for
[RFC 0006](../../../rfcs/0006-extended-attributes-over-64-kib.md), which proposes storing
extended attribute values over voidfs's 64 KiB limit as content.

APFS on this Mac sets and reads back a `com.apple.ResourceFork` and an ordinary `user.big`
attribute of every size tried, from 65,537 bytes to 1 GiB, and reports
`_PC_XATTR_SIZE_BITS` = 56 ([output](raw/apfs-xattr-sizes.txt)). Any limit voidfs sets is
therefore a choice, not something the Mac imposes. A value over voidfs's limit fails where APFS
would accept it.

The probe uses positionless `fsetxattr` and `fgetxattr`, one value at a time. It does not show how
Finder or `copyfile` move values of these sizes, and it does not touch FSKit. FSKit was measured
separately the same day ([vishnu597/voidfs#71](https://github.com/vishnu597/voidfs/pull/71)): it
passes a module values up to 262,143 bytes whole and refuses larger ones with `EFBIG`, unlike APFS.

```sh
cc -std=c11 -Wall -Wextra -Werror -O2 -o /tmp/apfs-xattr-sizes bench/results/xattr-limits/apfs-xattr-sizes.c
/tmp/apfs-xattr-sizes /tmp/apfs-xattr-sizes-target.bin
```

The target must not exist; the probe deletes it at the end. It needs about 1 GiB of memory and,
briefly, of disk.

The 64 KiB failures this RFC addresses were measured earlier and are not repeated here:
[item 4's resource forks](../mac-fskit-writable/raw/resource-fork.txt) (12 October: `cp` of a
200 KiB fork exits 1 with `E2BIG`, and Finder's duplicate drops it without an error) and
[the application matrix's Finder row](../mac-app-matrix/README.md#environment-and-matrix)
(13 October: a 65,537-byte fork is dropped by successful Finder copies on FSKit and kept on APFS).
