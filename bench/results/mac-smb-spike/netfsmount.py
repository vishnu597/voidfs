#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Mount an SMB URL the way Finder's Connect to Server does (NetFS's NetFSMountURLSync), with no
user interface, and print where it landed: shows whether a user can get an SMB drive into /Volumes
without a privileged helper.

    netfsmount.py smb://user@127.0.0.1:PORT/share <password file> [mountpoint dir]
"""

import ctypes
import ctypes.util
import sys

cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
netfs = ctypes.CDLL("/System/Library/Frameworks/NetFS.framework/NetFS")
kUTF8 = 0x08000100

cf.CFStringCreateWithCString.restype = ctypes.c_void_p
cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
cf.CFURLCreateWithString.restype = ctypes.c_void_p
cf.CFURLCreateWithString.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
cf.CFURLCreateWithFileSystemPath.restype = ctypes.c_void_p
cf.CFURLCreateWithFileSystemPath.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_bool]
cf.CFDictionaryCreateMutable.restype = ctypes.c_void_p
cf.CFDictionaryCreateMutable.argtypes = [ctypes.c_void_p, ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p]
cf.CFDictionarySetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
cf.CFArrayGetCount.restype = ctypes.c_long
cf.CFArrayGetCount.argtypes = [ctypes.c_void_p]
cf.CFArrayGetValueAtIndex.restype = ctypes.c_void_p
cf.CFArrayGetValueAtIndex.argtypes = [ctypes.c_void_p, ctypes.c_long]
cf.CFStringGetCString.restype = ctypes.c_bool
cf.CFStringGetCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32]
netfs.NetFSMountURLSync.restype = ctypes.c_int
netfs.NetFSMountURLSync.argtypes = [ctypes.c_void_p] * 6 + [ctypes.POINTER(ctypes.c_void_p)]


def s(text):
    return cf.CFStringCreateWithCString(None, text.encode(), kUTF8)


kCFTypeDictionaryKeyCallBacks = ctypes.c_void_p.in_dll(cf, "kCFTypeDictionaryKeyCallBacks")
kCFTypeDictionaryValueCallBacks = ctypes.c_void_p.in_dll(cf, "kCFTypeDictionaryValueCallBacks")
kCFBooleanTrue = ctypes.c_void_p.in_dll(cf, "kCFBooleanTrue")

url = cf.CFURLCreateWithString(None, s(sys.argv[1]), None)
password = open(sys.argv[2]).read().strip()
mountpath = cf.CFURLCreateWithFileSystemPath(None, s(sys.argv[3]), 0, True) if len(sys.argv) > 3 else None
open_opts = cf.CFDictionaryCreateMutable(None, 0, ctypes.addressof(kCFTypeDictionaryKeyCallBacks), ctypes.addressof(kCFTypeDictionaryValueCallBacks))
cf.CFDictionarySetValue(open_opts, s("UIOption"), s("NoUI"))
# NetFS refuses a server on this Mac unless asked (kNetFSAllowLoopbackKey).
cf.CFDictionarySetValue(open_opts, s("AllowLoopback"), kCFBooleanTrue)
# Leave the user's network preferences alone (kNetFSNoUserPreferencesKey).
cf.CFDictionarySetValue(open_opts, s("NoUserPreferences"), kCFBooleanTrue)
mount_opts = cf.CFDictionaryCreateMutable(None, 0, ctypes.addressof(kCFTypeDictionaryKeyCallBacks), ctypes.addressof(kCFTypeDictionaryValueCallBacks))
points = ctypes.c_void_p()
user = sys.argv[1].split("//", 1)[1].split("@", 1)[0]
rc = netfs.NetFSMountURLSync(url, mountpath, s(user), s(password), open_opts, mount_opts, ctypes.byref(points))
print(f"NetFSMountURLSync returned {rc}")
if rc == 0 and points.value:
    for i in range(cf.CFArrayGetCount(points)):
        buf = ctypes.create_string_buffer(1024)
        cf.CFStringGetCString(cf.CFArrayGetValueAtIndex(points, i), buf, 1024, kUTF8)
        print("mounted at", buf.value.decode())
