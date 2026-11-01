-- What students see comes only from the tests (@visibility): course
-- visibility overrides are gone, and with them what courses had set.
ALTER TABLE assignments DROP COLUMN visibility_overrides;
