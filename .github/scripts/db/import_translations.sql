-- Import historical translation data from ci-data/translations.csv into the DB.
-- Run from the repo root so relative path resolves correctly.
\copy translations (country_code, file_name, translation_key, status, git_commit)
  FROM 'ci-data/translations.csv' CSV HEADER;
