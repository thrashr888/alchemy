# Spring 2026 Customer Interviews — Synthesis

**Author:** Maya Chen (PM, Platform) · **Interviews:** 14 customers, April 6 – May 22, 2026 · **Method:** 45-minute semi-structured calls, two researchers per call

## Top-line finding

Platform teams love Timberline's service catalog, but **environment provisioning is where trust breaks down**. Developers route around the platform when a staging environment takes days, and every workaround becomes a compliance liability the platform team inherits.

## Pain points, ranked by frequency

### 1. Environment provisioning is too slow (11 of 14 customers)
> "A new hire can ship to production faster than they can get a staging environment. That's backwards." — Director of Platform Engineering, **Meridian Bank**

> "We measured it: median four days from Jira ticket to usable environment. Two of those days are just waiting for someone to notice the ticket." — Staff Engineer, **Vantage Logistics**

### 2. Compliance evidence collection is manual (9 of 14)
> "Every SOC 2 audit, I lose two engineers for three weeks to screenshot archaeology. If Timberline exported an evidence bundle, I'd expand seats tomorrow." — VP Engineering, **Cascade Health**

### 3. Staging drifts from production (8 of 14)
> "Staging is a lie. It was cloned from prod in 2024 and they've been growing apart ever since." — Principal Engineer, **Orbital Media**

### 4. No SSO is a dealbreaker for security teams (7 of 14)
> "I can't roll this out to 400 developers on username/password. My CISO would walk me out of the building." — Platform Lead, **Northwind Grid**

### 5. Onboarding new developers takes too long (6 of 14)
> "Time-to-first-deploy for a new dev is about three weeks. Half of that is environment access and tribal knowledge." — Engineering Manager, **Meridian Bank**

## What customers would pay for

- Meridian Bank: +120 seats if SSO and audit export land by October (their fiscal deadline)
- Cascade Health: expansion contingent on SOC 2 evidence bundle
- Northwind Grid: wants self-hosted; explicitly said "we will wait one more quarter, not two"

## Verbatim worth remembering

> "Timberline is the first platform tool my developers didn't immediately try to escape." — CTO, Orbital Media

> "Blueprints, if they work as pitched, replace our internal wiki, our Terraform modules repo, and about six Slack channels of pleading." — Vantage Logistics
