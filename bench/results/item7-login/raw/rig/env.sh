# The login and reboot check's rig (item 7). Source it: . ~/.voidfs-login-rig/env.sh
R=$HOME/.voidfs-login-rig
APP=/Applications/voidfs.app
V=$APP/Contents/MacOS/voidfs
VOID=$APP/Contents/MacOS/void
D=$R/state            # the app's state directory (VoidfsStateDirectory) for this check
C=$R/cli-state        # another client's state directory: foreground uploads, no daemon
EV=~/Coding/voidfs/bench/results/item7-login/raw
EP=http://127.0.0.1:9372
DRIVE=login-test
MNT=$HOME/voidfs/login-test
LSR=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
