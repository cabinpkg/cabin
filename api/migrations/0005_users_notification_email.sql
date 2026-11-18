-- The account's notification address: the verified primary e-mail of
-- its GitHub account, read at sign-in (src/notification_email.rs).
-- NULL until a sign-in reads one, and again once GitHub reports none
-- the registry can use. Never an identity: accounts stay keyed by the
-- numeric GitHub id in identities.
ALTER TABLE users ADD COLUMN notification_email TEXT;
