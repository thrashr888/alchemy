# Support Escalations — June 2026

**Owner:** Support Engineering (Rosa Delgado) · Distributed to product + eng leads July 2

## Volume overview

2,118 tickets in June (up 8% MoM, roughly tracking WAU growth). 41 escalations reached engineering, 3 became incidents. CSAT 4.6/5, flat.

## Top drivers

### 1. Environment request delays — 31% of tickets
Same story as May, louder. Developers file a ticket when their environment request sits in the platform-team queue; our support team can only shrug and forward it. **This is not a defect — it is the product gap Blueprints closes.** Recommend a canned response pointing to the 2.0 beta waitlist; drafted, needs PM sign-off.

### 2. Permissions confusion — 18% of tickets
"Why can't I see this service?" in various forms. Root cause is our four hardcoded roles not matching real org structures. The 2.0 role-mapping work should cut this, but note: **most of these tickets come from Team-tier accounts that will not get SSO/role-mapping under current packaging.** Flagging for pricing review.

### 3. Terraform state conflicts on shared environments — 11%
Two developers apply against the same shared staging environment and lock or corrupt state. Workaround doc gets heavy traffic. Blueprints' per-developer environments eliminate the sharing that causes this.

## Incidents

- **June 9 — catalog search degraded 40 min.** OpenSearch node failure; auto-recovery worked, alerting paged late. Action: page threshold fixed.
- **June 17 — webhook delivery delayed up to 25 min** for ~200 customers after a deploy. Rollback clean. Postmortem action: canary webhooks before full rollout (owner: Elena, due July 22).
- **June 24 — SSO preview environment outage (design partners only).** Okta cert rotation missed. Embarrassing given the audience; runbook updated.

## Notable verbatims

> "Your product is great at telling me what exists and terrible at giving me one of them." — Team-tier customer, ticket #48122

> "We bought Timberline to get rid of tickets and now I file tickets about tickets." — Enterprise prospect in POC, escalation #E-411 (they signed anyway)

## Asks of product

1. PM sign-off on the Blueprints-waitlist canned response (Maya)
2. Decision on whether role-mapping lands anywhere below Enterprise (Priya — pricing)
3. Public status page component for environment-provisioning queue depth (nice-to-have)
