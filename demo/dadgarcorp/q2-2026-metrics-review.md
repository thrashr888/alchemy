# Q2 2026 Metrics Review — Timberline

**Prepared by:** RevOps (Dana Ferreira) for the July 8 board pre-read · **Period:** April 1 – June 30, 2026

## Headline numbers

| Metric | Q2 2026 | Q1 2026 | Δ |
| --- | --- | --- | --- |
| ARR | $38.2M | $34.9M | +9.5% |
| Net revenue retention | 118% | 121% | −3 pts |
| Gross logo retention | 94.1% | 95.0% | −0.9 pts |
| Weekly active developers | 61,400 | 54,800 | +12% |
| NPS | 41 | 44 | −3 |
| Median activation time (signup → first deploy) | 9 days | 11 days | −2 days |

## What's working

- **Team tier self-serve is compounding.** 46% of new ARR came in without a sales touch, up from 38%. The pricing change in March (Team at $29/dev/mo) is doing what we hoped.
- **Activation improved** two days quarter-over-quarter, driven by the guided first-deploy flow shipped in April. Cohorts that complete first deploy inside 7 days retain at 97% after 90 days versus 81% for those who don't.

## What's not

- **NPS slipped 3 points.** Detractor comments cluster on two themes: environment provisioning wait times (34% of detractor verbatims) and missing SSO (22%). Both are Q3 roadmap items; the risk is churn before we ship.
- **Two logo churns in June** — Halycon Apps (startup, ran out of funding) and Brightbeam (lost to DIY Backstage build; cited "we needed SSO and you didn't have it" in the exit interview).
- **Enterprise pipeline is gated.** Danny's team reports 9 opportunities worth $2.4M ARR blocked on SSO/SCIM and audit logs. Average enterprise sales cycle is now 127 days, up from 98 — security review is the long pole.

## Support signal (from June escalation report)

Top three drivers of support volume: environment request delays (31% of tickets), permissions confusion (18%), Terraform state conflicts on shared environments (11%). Full detail in the June support escalations report.

## Recommendation

Hold the September 15 GA date for Timberline 2.0. Every week of slip has a measurable cost: Meridian's 120-seat expansion has an October fiscal deadline, and NPS detractors are telling us exactly what they'll churn over.
