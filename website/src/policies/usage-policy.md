# Usage Policy

**Short version:** *The Cabin registry is a shared resource for the C/C++
community, hosting a variety of packages from a diverse group of users. That
resource is only effective when our users are able to work together as part of
a community in good faith. While using the Cabin registry, you must comply with
our Acceptable Use Policies, which include some restrictions on content and
conduct related to user safety, intellectual property, privacy, authenticity,
and other limitations. In short, be excellent to each other!*

This policy applies to the Cabin registry service as a whole: the machine read
plane at `registry.cabinpkg.com` and the website and API at `cabinpkg.com`.
Cabin is pre-1.0 and the registry is currently in a limited-access phase;
these policies apply to all users of the service regardless of how access was
granted.

## Acceptable Use

We do not allow content or activity on the Cabin registry that:

- is unlawful or promotes unlawful activities
- is libelous, defamatory, or fraudulent
- amounts to phishing or attempted phishing
- infringes any proprietary right of any party, including patent, trademark,
  trade secret, copyright, right of publicity, or other right
- unlawfully shares unauthorized product licensing keys, software for
  generating unauthorized product licensing keys, or software for bypassing
  checks for product licensing keys, including extension of a free license
  beyond its trial period
- contains malicious code, such as computer viruses, computer worms, rootkits,
  back doors, or spyware, including content submitted for research purposes
  (tools designed and documented explicitly to assist in security research are
  acceptable, but exploits and malware that use the Cabin registry as a
  deployment or delivery vector are not)
- uses obfuscation to hide or mask functionality
- is discriminatory toward, harasses or abuses another individual or group
- threatens or incites violence toward any individual or group, especially on
  the basis of who they are
- is using the Cabin registry as a platform for propagating abuse on other
  platforms
- violates the privacy of any third party, such as by posting another person's
  personal information without consent
- gratuitously depicts or glorifies violence, including violent images
- is sexually obscene or relates to sexual exploitation or abuse, including of
  minors (see "Sexually Obscene Content" below)
- is off-topic, or interacts with platform features in a way that
  significantly or repeatedly disrupts the experience of other users
- exists only to reserve a name for a prolonged period of time (often called
  "name squatting") without having any genuine functionality, purpose, or
  significant development activity
- is related to buying, selling, or otherwise trading of package names,
  scopes, or any other names on the Cabin registry for money or other
  compensation
- impersonates any person or entity, including through false association with
  the Cabin project, or by fraudulently misrepresenting your identity or
  site's purpose
- is related to inauthentic interactions, such as fake accounts and automated
  inauthentic activity
- is using our servers for any form of excessive automated bulk activity, to
  place undue burden on our servers through automated means, or to relay any
  form of unsolicited advertising or solicitation through our servers, such as
  get-rich-quick schemes
- is using our servers for other automated excessive bulk activity or
  coordinated inauthentic activity, such as
  - spamming
  - cryptocurrency mining
- is not functionally compatible with the `cabin` build tool (for example, a
  "package" cannot simply be a PNG or JPEG image, a movie file, or a text
  document uploaded directly to the registry)
- is abusing the package index for purposes it was not intended

You are responsible for using the Cabin registry in compliance with all
applicable laws, regulations, and all of our policies. These policies may be
updated from time to time. We will interpret our policies and resolve disputes
in favor of protecting users as a whole. The Cabin maintainers reserve the
possibility to evaluate each instance on a case-by-case basis.

For issues such as DMCA takedown requests, or trademark and copyright
disputes, please contact us as described in the "Reporting" section below.
Such requests are reviewed by the Cabin maintainers on a case-by-case basis.

## Scopes and Package Ownership

All package names on the Cabin registry are scoped: a package is always named
`<scope>/<name>` (for example, `fmtlib/fmt`), and there is no bare-name or
alias mechanism.

A scope is claimed by proving control of the GitHub user or organization
account with the same name. A user scope can only be claimed by the matching
GitHub user; an organization scope can only be claimed by an active member of
the matching organization with the admin role. Because a claim is bound to the
GitHub account's numeric ID at claim time, renaming or recycling a GitHub
login later does not transfer or release a claimed scope. There is no
reservation list, and scope disputes are handled manually by the Cabin
maintainers.

Package names within a scope are managed by the members of that scope. Scope
owners can add and remove members, and publishing and yanking are authorized
by scope membership. If you want to take over a package, we require you to
first try and contact the members of its scope directly. If they agree, they
can add you to the scope, or publish the package under a scope you control. If
the current members are not reachable, the Cabin maintainers may help mediate
the process.

Deleting a published package or version is not possible, in order to keep the
registry as immutable as possible. If a release should no longer be used, you
can yank it: yanked versions remain downloadable for existing lockfiles, but
are no longer selected when resolving new dependencies.

The Cabin maintainers may remove packages or scopes from the registry that do
not comply with the policies in this document. In severe cases, such as
coordinated abuse, this may happen without prior notification to the author,
but in most cases the maintainers will first give the author the chance to
justify the purpose of the package.

## Data Access

If you need access to package metadata in bulk, please use the sparse HTTP
index served at `registry.cabinpkg.com`. It is the same read plane the
`cabin` client uses: `config.json`, per-package metadata files, and package
archives. Please honor HTTP caching headers when crawling it. We do not
currently publish database dumps; if the index does not cover your use case,
please open a thread on our [GitHub
Discussions](https://github.com/orgs/cabinpkg/discussions).

You may also use the registry API under `cabinpkg.com/api` directly, though
excessive usage may be blocked at our discretion. We require users of the API
to limit themselves to a maximum of 1 request per second.

We also require all API users to provide a `User-Agent` header that allows us
to uniquely identify your application. This allows us to more accurately
monitor any impact your application may have on our service. Providing a user
agent that only identifies your HTTP client library (such as `reqwest/0.12.5`)
increases the likelihood that we will block your traffic.

It is recommended to include contact information in your `User-Agent` header:

- Bad: `User-Agent: reqwest/0.12.5`
- Better: `User-Agent: my_bot`
- Best: `User-Agent: my_bot (my_bot.com/info)` or
  `User-Agent: my_bot (help@my_bot.com)`

This allows us to contact you if we would like a change in your application's
behavior without having to block your traffic.

The Cabin registry runs on shared, cost-limited infrastructure with per-user
quotas and rate limits, and automated protections may throttle or block
traffic that threatens the availability of the service for others. We reserve
the right to block traffic from any client that we determine to be in
violation of this policy or causing an impact on the integrity of our service.

## Security

We would like to ensure that both the `cabin` client and the hosted registry
have secure implementations. To report a security vulnerability in either,
please open a private report at
<https://github.com/cabinpkg/cabin/security/advisories/new>. See our [Security
Policy](https://github.com/cabinpkg/cabin/blob/main/SECURITY.md) for the exact
scope.

Note that this policy only applies to Cabin itself, and not to individual
packages hosted on the registry. The Cabin maintainers are not responsible for
the disclosure of vulnerabilities in specific packages; if you find an issue
in a package, you should seek guidance from the members of its scope and their
specific policies instead.

Thank you for taking the time to responsibly disclose any issues you find.

## Sexually Obscene Content

We do not tolerate content associated with sexual exploitation or abuse of
another individual, including where minors are concerned. We do not allow
sexually themed or suggestive content that serves little or no purpose other
than to solicit an erotic or shocking response, particularly where that
content is amplified by its placement in profiles or other social contexts.

This includes:

- Pornographic content
- Non-consensual intimate imagery
- Graphic depictions of sexual acts including photographs, video, animation,
  drawings, computer-generated images, or text-based content

We recognize that not all nudity or content related to sexuality is obscene.
We may allow visual and/or textual depictions in artistic, educational,
historical or journalistic contexts, or as it relates to victim advocacy. In
some cases a disclaimer can help communicate the context of the project.

## Violations and Enforcement

The Cabin maintainers retain full discretion to take action in response to a
violation of these policies, including account suspension, account
termination, or removal of content.

We will however not be proactively monitoring the site for these kinds of
violations, but instead relying on the community to draw them to our
attention.

While the majority of interactions between individuals in the Cabin community
fall within our policies, violations of those policies do occur at times. When
they do, the Cabin maintainers may need to take enforcement action to address
the violations. In all cases, content and account deletion is permanent and
there is no basis to reverse these moderation actions. Account suspension may
be lifted at the maintainers' discretion, however, for example in the case of
someone's account being compromised.

## Reporting

Please report violations of this policy through GitHub. Note that both
channels below require a GitHub account; since the registry itself uses GitHub
sign-in, this should not be an additional barrier for most users.

- If your report only involves information that is already public or can be
  shared publicly — for example, spam, name squatting, name trading, or
  impersonation — please open an issue on our [issue
  tracker](https://github.com/cabinpkg/cabin/issues).
- If your report involves suspected malware, sensitive material, another
  person's private information, or anything else that should not be posted
  publicly, please use our [private reporting
  form](https://github.com/cabinpkg/cabin/security/advisories/new) instead,
  even if the report is not a vulnerability in Cabin itself. This also applies
  to legal requests such as DMCA takedown notices.

When in doubt, use the private reporting form.

## Credits & License

This policy is partially based on [crates.io Use Policy](https://crates.io/policies)
and modified from its original form.

Licensed under the [Creative Commons Attribution 4.0 International
license](https://creativecommons.org/licenses/by/4.0/).
