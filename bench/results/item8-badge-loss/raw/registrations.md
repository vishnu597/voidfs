# Registrations

[snapshot.sh](../../item8-live/raw/live/setup/snapshot.sh) recorded the Finder Sync and FSKit plug-ins,
the `dev.voidfs.app` defaults, the agent's launchd job, mounts, LaunchServices' voidfs paths and
voidfs processes:

- [before](registrations/registrations-before.txt), before the first build;
- [during](registrations/registrations-during.txt), after the runs. Building added this build's
  `voidfs.app`, `VoidfsFS.appex` and `VoidfsFinder.appex` to LaunchServices. `pluginkit` didn't
  list them;
- [after](registrations/registrations-after.txt), after `lsregister -u` on this build. It is the
  same as before. The build's products were then deleted.

Nothing else was registered, enabled or written: no FSKit module, agent, Finder extension or
default changed, nothing was mounted, and `~/Library/Application Support/voidfs` was absent
throughout ([rig-down](setup/rig-down.txt)).
