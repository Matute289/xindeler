-- Export all benchmark data from DB back to ci-data/benchmarks.csv.
\copy (
  SELECT "group", "function", value, throughput_num, throughput_type,
         sample_measured_value, unit, iteration_count, git_commit, branch, recorded_at
  FROM benchmarks
  ORDER BY recorded_at DESC, "group", "function"
) TO 'ci-data/benchmarks.csv' CSV HEADER;
