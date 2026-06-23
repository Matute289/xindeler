-- BL-33: per-character moral alignment (comp::Ethos), stored as two signed
-- scores (-100..100). Default 0/0 = True Neutral, so existing characters load
-- as True Neutral. The discrete 9-box (Good/Neutral/Evil x Lawful/Neutral/
-- Chaotic) is derived in code from these scores.
ALTER TABLE "character" ADD COLUMN ethos_good_evil INTEGER NOT NULL DEFAULT 0;
ALTER TABLE "character" ADD COLUMN ethos_law_chaos INTEGER NOT NULL DEFAULT 0;
