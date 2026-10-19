# Native Finder observations

Recorded by the operator from the computer-use and shell tool results on 19 October 2026.
This is an observation note, not a Finder diagnostic log.

1. `finder-check.py hold` mounted the rig, took its proxy offline and acknowledged a
   131,111-byte save while keeping its writable descriptor open.
2. In Finder, a new window opened the rig mount through Go to Folder. File → Eject was
   selected. Computer view then showed `busy-eject-oct19` as a Remote Volume with an eject
   button. That button and the volume's context-menu Eject were also selected. No dialog
   appeared; the volume remained in Computer view.
3. An independent `/sbin/mount` command after the native eject attempts still printed:

   ```text
   voidfs://busy-eject-oct19 on /Users/vishnu/.voidfs-eject-rig-oct19/mnt (voidfs, nodev, nosuid, noowners, noatime, fskit, mounted by vishnu)
   ```

4. The holder was released and exited successfully. Selecting the same Computer-view eject
   button then removed the Remote Volume row; Finder's item count changed from three to two.
   A separate Python command asserted the exact resource/mountpoint line was absent from
   `/sbin/mount` output before recording `finder-normal-eject-after-close-succeeded`.
5. The daemon's mount-intent table still listed the ejected volume, as it does after diskutil
   eject. `finder-check.py recover` reconciled it with ordinary `void unmount`, remounted
   offline, verified the saved bytes, restarted the daemon offline, verified again and then
   compared the published bytes from the independent peer. See `finder-recovery.txt`.

No Finder force choice or force dialog was used. Daemon mount status alone cannot establish
whether Finder actually ejected the OS volume; the independent mount table and native UI above
are the evidence for the busy/no-handle distinction.
