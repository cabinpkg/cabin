-- The foundation ports and their publish workflow moved from
-- cabinpkg/cabin to cabin-ports/ports (.github/workflows/publish.yml):
-- rebind the operator-seeded cabin-ports config to that repository and
-- workflow, still from main only. The numeric ids are cabin-ports/ports
-- (1446491029) and the cabin-ports organization (308231483). Updating
-- the one seeded row in place, rather than adding a second, leaves
-- exactly one repository able to publish the scope. An exchange that
-- read the old row before this UPDATE can still mint, and minted tokens
-- live out their TTL: apply it only once the old workflow is disabled,
-- has no run in flight, and its last token has expired or been revoked.
UPDATE trustpub_configs
SET repository_owner_id = 308231483,
    repository_id = 1446491029,
    workflow_filename = 'publish.yml'
WHERE scope = 'cabin-ports'
  AND repository_owner_id = 35998702
  AND repository_id = 119684778
  AND workflow_filename = 'ports-publish.yml';
