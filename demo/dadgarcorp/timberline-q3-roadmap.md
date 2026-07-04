# Timberline Q3 2026 Roadmap

**Owner:** Product (Maya Chen) · **Status:** Approved at July planning · **Last updated:** July 1, 2026

Timberline 2.0 is DadgarCorp's biggest release since the original launch. The theme for Q3 is **"Enterprise-ready golden paths"** — closing the gaps that stall our largest deals (SSO, audit, compliance evidence) while cutting environment provisioning time from days to minutes.

## Key dates

- **July 14** — Feature freeze for Environment Blueprints beta
- **July 28** — Private beta begins (12 design partners, including Meridian Bank and Cascade Health)
- **August 18** — SSO/SCIM reaches GA-quality bar; security review complete
- **September 2** — Release candidate; docs and pricing page updates locked
- **September 15** — **Timberline 2.0 general availability**
- **September 29** — Post-launch review; begin Q4 planning

## Workstreams

### 1. Environment Blueprints (flagship)
Terraform-native templates that let a platform team define a "golden path" environment once — service, database, queue, secrets, observability — and let any developer stamp out a compliant copy in under 10 minutes. Replaces the ticket-driven flow that today averages **3.2 days per environment request**.

- Blueprint authoring UI with drift detection
- Policy guardrails (cost caps, region allowlists, mandatory tags)
- One-click teardown with TTL defaults

### 2. Enterprise identity
- SAML SSO and SCIM provisioning (Okta and Entra ID first)
- Role mapping: platform-admin, blueprint-author, developer, auditor
- Blocks 9 open enterprise opportunities worth a combined **$2.4M ARR** (per June pipeline review)

### 3. Audit & compliance
- Immutable audit log with export to S3/Splunk
- SOC 2 evidence bundle: one-click export of access reviews and change history
- Compliance evidence collection is the #2 pain point from spring customer interviews

## Explicitly out of scope for Q3

- Self-hosted/air-gapped deployment (revisit in Q4 — Northwind Grid is asking)
- Windows runner support
- Cost analytics dashboards beyond basic spend caps

## Risks

1. **SSO security review depth.** Elena's team budgeted two weeks; the last review (webhooks, Q1) took four. Mitigation: external pentest booked for August 4, before the internal review.
2. **Design partner load.** Meridian's platform team wants weekly syncs; we've capped partner commitments at 6 hours/week of PM time.
3. **Blueprint migration.** 340 existing "legacy template" users need a migration path or 2.0 launches with a split experience.
