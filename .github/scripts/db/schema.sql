-- CI data schema for Veloren fork
-- Mirrors the structure used by the upstream cidb.veloren.net

CREATE TABLE IF NOT EXISTS translations_stage (
  country_code TEXT,
  file_name    TEXT,
  translation_key TEXT,
  status       TEXT,
  git_commit   TEXT
);

CREATE TABLE IF NOT EXISTS translations (
  country_code    TEXT,
  file_name       TEXT,
  translation_key TEXT,
  status          TEXT,
  git_commit      TEXT,
  loaded_at       TIMESTAMP DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS benchmarks (
  "group"                TEXT,
  "function"             TEXT,
  value                  NUMERIC,
  throughput_num         NUMERIC,
  throughput_type        TEXT,
  sample_measured_value  NUMERIC,
  unit                   TEXT,
  iteration_count        INTEGER,
  git_commit             TEXT,
  branch                 TEXT,
  recorded_at            TIMESTAMP DEFAULT NOW()
);

CREATE OR REPLACE PROCEDURE public.load_translations_from_stage()
LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO translations (country_code, file_name, translation_key, status, git_commit)
  SELECT country_code, file_name, translation_key, status, git_commit
  FROM translations_stage;
  TRUNCATE translations_stage;
END;
$$;
