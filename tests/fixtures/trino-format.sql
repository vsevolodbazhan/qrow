select
    model_reference,
    owner,
    team,
    count(*) as runs,
    count(application_id) as runs_with_application,
    round(sum(total_cost_usd), 2) as total_cost_usd,
    round(sum(used_cost_usd + unused_cost_usd), 2) as compute_cost_usd,
    round(sum(storage_cost_usd + iops_cost_usd + throughput_cost_usd), 2) as disk_cost_usd,
    round(sum(savings_plan_cost_usd), 2) as savings_plan_cost_usd,
    round(sum(total_cost_usd) / count(*), 2) as avg_run_cost_usd,
    round(sum(unused_cost_usd) / nullif(sum(used_cost_usd + unused_cost_usd), 0), 3) as unused_share
from sandbox_vbazhan.silver__dwh_meta__rivendell_models_cost_dedicated
where as_of between date'2026-10-21' and date'2026-10-25'
group by model_reference, owner, team
order by total_cost_usd desc;

select *
from sandbox_vbazhan.silver__dwh_meta__rivendell_models_cost_dedicated
where as_of between date'2026-10-21' and date'2026-10-25';

select
  as_of,
  team,
  round(sum(used_cost_usd + unused_cost_usd), 1) as compute_cost_usd
from sandbox_vbazhan.silver__dwh_meta__rivendell_pods_hourly_cost
where
  as_of >= date'2026-10-21'
  and team = 'DE'
group by as_of, team
order by as_of, team;
