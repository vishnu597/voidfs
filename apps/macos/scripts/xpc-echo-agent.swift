// SPDX-License-Identifier: Apache-2.0
//
// xpc-echo-agent: a launchd agent that echoes XPC messages, for the FSKit spike's measurement of
// extension-to-agent latency. Serves every mach service name given as an argument.
//
//     swiftc -O -o xpc-echo-agent ../Shared/XPCEcho.swift xpc-echo-agent.swift
//     launchctl bootstrap gui/$UID xpc-echo-agent.plist      (see the spike write-up)
//     launchctl bootout gui/$UID/dev.voidfs.xpc-echo

import Foundation

final class Echo: NSObject, XPCEcho {
    func echo(_ data: Data, reply: @escaping (Data) -> Void) { reply(data) }
}

final class Delegate: NSObject, NSXPCListenerDelegate {
    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection c: NSXPCConnection) -> Bool {
        c.exportedInterface = NSXPCInterface(with: XPCEcho.self)
        c.exportedObject = Echo()
        c.resume()
        return true
    }
}

let delegate = Delegate()
let listeners = CommandLine.arguments.dropFirst().map { name in
    let l = NSXPCListener(machServiceName: name)
    l.delegate = delegate
    l.resume()
    return l
}
RunLoop.main.run()
