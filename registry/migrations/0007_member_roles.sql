-- Close the member-role domain now that the claim flow and membership
-- management write it. The last-owner rule and the owner gate key on the
-- exact 'owner' spelling, and membership disputes are manual SQL
-- (docs/architecture.md, "Scopes") - the constraint keeps a hand-run
-- typo from silently widening access or orphaning a scope. Destructive
-- recreate on purpose: the registry is pre-launch and no scope rows
-- exist before the claim flow this ships with.

DROP TABLE scope_members;

CREATE TABLE scope_members (
    scope_name TEXT NOT NULL REFERENCES scopes,
    user_id INTEGER NOT NULL REFERENCES users,
    role TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    PRIMARY KEY (scope_name, user_id)
);
