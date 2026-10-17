import re, sys
p = sys.argv[1]
s = open(p).read()
start = s.index("    private func publish(_ now: [Mount]) {")
end = s.index("\n    }\n", start) + len("\n    }\n")
old = '''    private func publish(_ now: [Mount]) {
        let gone = Set(mounts.map(\\.mountpoint)).subtracting(now.map(\\.mountpoint))
        let changed = Set(mounts.map(\\.mountpoint)) != Set(now.map(\\.mountpoint))
        mounts = now
        for (key, f) in folders where gone.contains(key.mountpoint) {
            for url in f.asked.values { setBadge("", url) }
            if let top = f.top { setBadge("", top) }
            folders[key] = nil
        }
        if changed { setDirectories(Set(now.flatMap { Self.spellings($0.mountpoint) }.map { URL(filePath: $0, directoryHint: .isDirectory) })) }
    }
'''
s = s[:start] + old + s[end:]
open(p, "w").write(s)
