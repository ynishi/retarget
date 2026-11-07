# Security policy

retarget is maintained by one person. This document says where to send a vulnerability report, what happens
after you do, and what is in scope.

## Reporting

**Use GitHub's private vulnerability reporting on this repository** — the
"Report a vulnerability" button under the Security tab. It opens a private
thread visible only to the maintainer, so the report never sits in a public
issue while a fix is being written.

Please do not open a public issue for a suspected vulnerability. If private
reporting is unavailable to you for any reason, open a public issue that says
only that you have a security report and asks for a contact — no details.

What helps most in a report: what an attacker can reach, the steps to reproduce
it, and the version or commit you saw it on. A proof of concept is welcome but
not required.

## What happens next

This is a single-maintainer project, so these are commitments a single person
can keep rather than a team SLA:

| Stage                                                      | Target                                                              |
| ---------------------------------------------------------- | ------------------------------------------------------------------- |
| Acknowledge the report                                     | within 7 days                                                       |
| Initial assessment — is it a vulnerability, and how severe | within 30 days                                                      |
| Fix, or an explanation of why it will not be fixed         | discussed with you once assessed                                    |
| Public disclosure                                          | when the fix ships, or 90 days after the report, whichever is first |

The 90-day deadline is the industry norm and it runs regardless of whether a fix
is ready, because an unfixed issue that stays secret indefinitely protects
nobody but the project. If you need it held longer — a coordinated release with
another project, for example — say so and it can be agreed.

Reports are handled through GitHub security advisories, which means you are
credited on the published advisory unless you ask not to be. There is no bug
bounty; nothing here is offered in exchange for a report.

## Scope

In scope: behaviour of retarget itself that lets input it does not
control reach memory, resources, or data it should not — including a panic the
API does not document — and a dependency with a known vulnerability that
retarget actually exercises. For retarget in particular: writing a setting
outside the harness's project scope, or a telemetry receiver reachable from
another machine.

Out of scope: anything that assumes the attacker already controls the
application embedding retarget, and bugs in third-party software — report
those upstream.

## Safe harbour

Good-faith research on your own installation is welcome and will not be met with
legal action. "Good faith" means: your own data, no third party's, no attempt to
reach anyone else's machine, and no public disclosure before the window above
closes.

## Handling a report that turns out to be a disclosure

If a report shows that protected content is already published — a credential in
history, personal data in a fixture — it stops being a vulnerability report and
follows [After a disclosure](PUBLIC_DEVELOPMENT.md#after-a-disclosure), which is
where the containment and notification steps live.
