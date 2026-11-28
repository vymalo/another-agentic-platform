# Environments, storage, caches and worktrees — evidence

The page for people is [`docs/architecture/06-environments-and-storage.md`](../../../docs/architecture/06-environments-and-storage.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim.

## §29. Shared Project Storage

The first bullet of the storage reality check (netcup, 2026-09-28) read:

> The only StorageClasses are `longhorn` (default) and `longhorn-static`. Sharing one project volume between concurrently running agents needs ReadWriteMany; Longhorn provides RWX through an NFS share-manager, whose performance for Git object databases and build `target/` directories is unverified here. A pod with `hostUsers: false` cannot mount that RWX claim (`MOUNT_ATTR_IDMAP`, *verified 2026-10-06*), while a Longhorn RWO ext4 volume mounts idmapped, and the `longhorn` class keeps 2 replicas (*verified 2026-10-06*, the owner's probes); see AD-045 and §59b.

So: the idmapped-mount claim and the replica count were verified on 2026-10-06, the second against the owner's probes on the netcup cluster; the NFS share-manager's performance for Git object databases and build `target/` directories had not been verified.
