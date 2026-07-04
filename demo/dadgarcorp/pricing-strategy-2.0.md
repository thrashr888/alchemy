# Timberline 2.0 Pricing Strategy

**Owner:** Priya Raman (CPO) with Dana Ferreira (RevOps) · **Status:** Approved June 24, 2026 · **Confidential — internal only**

## Principles

1. **Land self-serve, expand with the platform team.** The Team tier is the wedge; Enterprise features should never leak down-tier.
2. **Price the outcome, not the infrastructure.** Customers measure us against days-of-waiting saved, not compute.
3. **No re-papering existing contracts at launch.** Current customers keep current pricing through their renewal.

## Tiers at 2.0 launch (September 2026)

| Tier | Price | Includes |
| --- | --- | --- |
| Free | $0, up to 5 developers | Service catalog, 2 blueprints, community support |
| Team | $29/dev/month | Unlimited blueprints, drift detection, standard support |
| **Pro** | **$79/dev/month** | Policy guardrails, TTL/teardown automation, priority support |
| Enterprise | Custom (floor $150k/yr) | SSO/SCIM, audit log export, SOC 2 evidence bundle, dedicated CSM |

The Pro tier is new at 2.0. Modeling (Dana, June 12 deck) shows 22% of Team accounts have >40 developers and hit guardrail needs; at $79 the upgrade pays for itself if it saves each developer 25 minutes a month.

## Packaging decisions

- **SSO is Enterprise-only.** This was debated hard — Marcus argued for SSO in Pro to defuse the "SSO tax" criticism. Decision: keep it Enterprise for 2.0, revisit if it shows up as a Pro-tier churn driver by Q4. The $2.4M blocked pipeline is all Enterprise-shaped anyway.
- **Audit log retention:** 30 days in Pro, 13 months in Enterprise.
- **Blueprints are unmetered in every paid tier.** Metering blueprint executions was modeled and rejected: it punishes exactly the behavior we want to encourage.

## Migration & grandfathering

- Legacy "Business" tier ($49/dev/mo, closed to new sales since March) maps to Pro at renewal with a 12-month price bridge at $59.
- The 340 legacy-template accounts get free migration tooling and 90 days of overlap.

## Open questions

- Annual-only for Enterprise, or allow quarterly for public-sector?
- Does the Free tier's 5-developer cap survive PLG review? Growth wants 10.
