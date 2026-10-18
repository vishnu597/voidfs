// SPDX-License-Identifier: Apache-2.0
//
// `voidfs agent-selftest`: the bridge agent's launch-refusal check against `launchctl print` as it
// was on 18 October (`bench/results/item7-login/raw/agent.txt`), and the rounds that register it
// again, with stand-ins for launchd and SMAppService. It registers nothing.

import Foundation

enum AgentSelfTest {
    /// The live run's first look at the agent, as `agent.txt` has it: an excerpt, so its
    /// requirement's block has no end.
    static let excerpt = """
        \tstate = not running
        \tprogram identifier = Contents/MacOS/voidfs (mode: 2)
        \tparent bundle identifier = dev.voidfs.app
        \tLWCR = {
        \t\t\t"signing-identifier" => "dev.voidfs.app"
        \t\t\t"validation-category" => 3
        \t\t\t"team-identifier" => "HAUTK68F56"
        \truns = 0
        \tlast exit code = (never exited)
        \tproperties = partial import | resolve program | has LWCR
        """

    /// The agent as `launchctl print` shows it, laid out as on this Mac, with the live run's state.
    /// Without `category`, its requirement names none.
    static func printed(category: Int?, state: String, runs: Int, exit: String = "(never exited)", team: String = "HAUTK68F56", after: String = "") -> String {
        """
        gui/501/dev.voidfs.agent = {
        \tactive count = 0
        \tpath = (submitted by smd.340)
        \ttype = Submitted
        \tmanaged_by = com.apple.xpc.ServiceManagement
        \tstate = \(state)

        \tprogram identifier = Contents/MacOS/voidfs (mode: 2)
        \tparent bundle identifier = dev.voidfs.app
        \tparent bundle version = 1
        \targuments = {
        \t\tvoidfs
        \t\tagent
        \t}

        \tLWCR = {
        \t\t"reqs" => {
        \t\t\t"signing-identifier" => "dev.voidfs.app"
        \(category.map { "\t\t\t\"validation-category\" => \($0)\n" } ?? "")\t\t\t"team-identifier" => "\(team)"
        \t\t}
        \t\t"vers" => 1
        \t\t"comp" => 1
        \t\t"ccat" => 0
        \t}
        \(after)
        \tdomain = gui/501 [100020]
        \truns = \(runs)
        \tlast exit code = \(exit)

        \tendpoints = {
        \t\t"HAUTK68F56.dev.voidfs.agent" = {
        \t\t\tactive = 1
        \t\t\tmanaged = 1
        \t\t}
        \t}

        \tresource coalition = {
        \t\ttype = resource
        \t\tstate = active
        \t}
        \tjob state = \(state)
        \tproperties = partial import | resolve program | has LWCR
        }
        """
    }

    static func run() async -> Int32 {
        var failed = 0
        func check(_ ok: Bool, _ what: String) {
            print("\(ok ? "ok  " : "FAIL") \(what)")
            if !ok { failed += 1 }
        }
        let developerID = LaunchRequirement.Signature(identifier: "dev.voidfs.app", team: "HAUTK68F56", category: 6)
        let development = LaunchRequirement.Signature(identifier: "dev.voidfs.app", team: "HAUTK68F56", category: 3)
        func refusal(_ printed: String, _ own: LaunchRequirement.Signature? = developerID) -> String? {
            LaunchRequirement.refusal(in: printed, own: own)
        }
        let kept = "its launch requirement names validation category 3 (development), not 6 (Developer ID)"
        // The live run, in order: the first look, after one round, after bridge-check, after two.
        let first = excerpt
        let afterOne = printed(category: 3, state: "spawn scheduled", runs: 1)
        let afterCheck = printed(category: 3, state: "spawn scheduled", runs: 3, exit: "78: EX_CONFIG")
        let afterTwo = printed(category: 6, state: "running", runs: 1)

        // The check, for the Developer ID copy in /Applications.
        let r = refusal(first)
        check(r == kept, "the live run's agent kept a development build's requirement, the right identifier with the wrong category: \(r ?? "none")")
        check(refusal(afterOne) == kept, "registered again once, launchd scheduled it with that requirement still")
        check(refusal(afterCheck) == kept, "which ended in exit code 78; the requirement is named first")
        check(refusal(afterTwo) == nil, "registered again twice, it runs with its own requirement")
        check(refusal(printed(category: 6, state: "not running", runs: 0)) == nil, "idle with its own requirement, an agent launchd hasn't started yet isn't refused")
        check(refusal(printed(category: 6, state: "not running", runs: 2, exit: "0")) == nil, "nor after a clean exit")
        let other = refusal(printed(category: 6, state: "not running", runs: 0, team: "ABCDE12345"))
        check(other == "its launch requirement names team ABCDE12345, not HAUTK68F56", "another team's requirement is: \(other ?? "none")")
        let backwards = refusal(printed(category: 6, state: "spawn scheduled", runs: 1), development)
        check(backwards == "its launch requirement names validation category 6 (Developer ID), not 3 (development)", "so is a Developer ID requirement kept for a development build, as on 9 October: \(backwards ?? "none")")
        check(refusal(printed(category: 3, state: "running", runs: 1)) == nil, "a running agent isn't, whatever its requirement: another copy's program may be running")
        check(refusal(first, LaunchRequirement.Signature(identifier: "dev.voidfs.app", team: "HAUTK68F56")) == nil, "without this binary's category, the category isn't compared")
        check(refusal(first, nil) == nil && refusal(afterCheck, nil) == "it can't start the program: exit code 78, EX_CONFIG", "unsigned, only exit code 78 counts")
        // A requirement that names no category, then another block that does.
        let outside = printed(category: nil, state: "not running", runs: 0, after: "\tother = {\n\t\t\"validation-category\" => 3\n\t}")
        check(refusal(outside) == nil, "a validation category after the requirement's block doesn't count")

        // This binary's signature, which the agent's program has.
        let own = LaunchRequirement.own
        check(own?.identifier == "dev.voidfs.app" && own?.team == "HAUTK68F56" && (own?.category == 3 || own?.category == 6),
              "this binary is the agent's program, signed for development or Developer ID: \(describe(own))")

        // The rounds, with stand-ins: `states[n]` is launchd's view after n rounds.
        func rounds(_ states: [String], failing: Int? = nil) async -> (Agent.Repair?, Int) {
            var registered = 0
            let repair = await Agent.repair(refused: { refusal(states[min(registered, states.count - 1)]) }, registerAgain: {
                registered += 1
                return registered != failing
            })
            return (repair, registered)
        }
        do {
            let (repair, registered) = await rounds([first, afterOne, afterTwo])
            check(repair == Agent.Repair(refused: kept, rounds: 2) && registered == 2, "the live run: kept after one round, it is registered again a second time and accepted (registered \(registered))")
            let note = repair?.note ?? ""
            check(note == "launchd refused the bridge agent (\(kept)), so it was registered again twice.", "the note says so: \(note)")
        }
        do {
            let (repair, registered) = await rounds([first, afterTwo])
            check(repair == Agent.Repair(refused: kept, rounds: 1) && registered == 1, "cleared by one round, it registers it once")
            check(repair?.note == "launchd refused the bridge agent (\(kept)), so it was registered again.", "and says so")
        }
        do {
            let (repair, registered) = await rounds([afterTwo])
            check(repair == nil && registered == 0, "accepted, it registers nothing again")
        }
        do {
            let (repair, registered) = await rounds([first, afterOne, afterCheck, afterCheck])
            check(repair == Agent.Repair(refused: kept, rounds: 2, still: kept) && registered == 2, "refused still, it is registered again only twice (registered \(registered))")
            check(repair?.note.contains("It was registered again twice, and launchd still refuses it (\(kept))") == true && repair?.fixed == false, "and it says launchd still refuses it: \(repair?.note ?? "none")")
        }
        do {
            let (repair, registered) = await rounds([first, afterOne, afterTwo], failing: 1)
            check(repair == Agent.Repair(refused: kept, rounds: 1, failed: true) && registered == 1, "if registering again fails, it stops and says so: \(repair?.note ?? "none")")
        }

        // This Mac, read only.
        print("this binary: \(describe(own))")
        if FileManager.default.fileExists(atPath: "/Applications/voidfs.app") {
            print("/Applications/voidfs.app: \(describe(LaunchRequirement.signature(of: URL(filePath: "/Applications/voidfs.app"))))")
        }
        let why = Agent.service.status == .enabled ? await Agent.refused() : nil
        print("this Mac: the agent is \(Agent.status); launchd \(why.map { "refuses it: \($0)" } ?? "doesn't refuse it")")
        print(failed == 0 ? "agent self-test passed" : "agent self-test: \(failed) failed")
        return failed == 0 ? 0 : 1
    }

    static func describe(_ s: LaunchRequirement.Signature?) -> String {
        guard let s else { return "unsigned" }
        return "\(s.identifier), team \(s.team ?? "none"), validation category \(s.category.map { LaunchRequirement.describe(category: $0) } ?? "not known")"
    }
}
