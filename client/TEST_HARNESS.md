# Admin test-character harness (`test_harness`)

A small headless client that batch-creates/configures **test characters** from a
RON roster, to test attunement and other mechanics without grinding. It's a **dev
tool**: it only drives the server's admin commands (`/make_test_char`) — it never
touches the save database.

## Security

The account you log in as **must be an admin** on the target server. The server
gates `/make_test_char` (`needs_role: Admin` + a `real_role()` re-check), so a
non-admin account is rejected server-side. The harness has no special power of its
own. Don't commit credentials.

## Usage

```bash
# 1. Copy and edit the roster (the real one is git-ignored)
cp client/roster.example.ron client/roster.ron

# 2. Run against a server where <admin> is an admin account
cargo run --bin test_harness --features "bin_bot,tick_network" -- \
  --username <admin> --server localhost --roster client/roster.ron
# (use --password <pw> for an auth server; omit for a --no-auth server)
```

For each roster entry the harness connects, creates the character if it doesn't
exist (the **race** becomes the humanoid species at creation), selects it, then
issues `/make_test_char <level> [class] [kit]` to set the rest. One connection per
entry (robust; avoids the back-to-character-select dance). It prints a per-entry
`✓`/`✗` summary.

## Roster format

See `roster.example.ron`. Per character: `name` (required), `level` (1–60,
required), optional `class` (warrior|mage|cleric|rogue), `race`
(human|orc|elf|dwarf|danari|draugr), and `kit` (any kit in
`server.manifests.kits`, e.g. `all`).

## Notes

- Characters are created as a Human **Warrior** with the always-valid sword starter;
  the real `class` (and `race` via creation body) come from the roster. The class is
  then force-set by `/make_test_char`.
- `race` is a creation-time attribute, so it's applied when the character is created
  (not changeable on an existing character via the command).
