# Where to enforce: evidence review for MAE's permission-tier ADR

**Bottom line up front.** Your decision contains two claims that the evidence treats very differently.

- **Claim A — "enforce at the effect, not at the interface/name."** Overwhelmingly **SUPPORTED**. Every mature system I checked places the check at the resource, and several explicitly rejected the name/API surface as a mediation point after trying it.
- **Claim B — "therefore gating the process-spawning primitive is sufficient; `eval_scheme` can stay at write tier."** **CONTRADICTED**, on two independent grounds: completeness (a gate on one primitive is a deny-list of one) and ambient authority (the thing your primitive consults is exactly what the confused-deputy literature identifies as the root cause). Deno states in its own docs that this is not achievable; Oracle and Microsoft both abandoned production systems built on it.

You are right about the enforcement point and wrong about what that buys you.

---

## 1. Saltzer & Schroeder (1975)

Verbatim from the [MIT-hosted text](https://web.mit.edu/Saltzer/www/publications/protection/Basic.html), §3 "Design Principles":

> **c) Complete mediation:** *Every access to every object must be checked for authority.* This principle, when systematically applied, is the primary underpinning of the protection system. It forces a system-wide view of access control, which in addition to normal operation includes initialization, recovery, shutdown, and maintenance. **It implies that a foolproof method of identifying the source of every request must be devised.** It also requires that proposals to gain performance by remembering the result of an authority check be examined skeptically. If a change in authority occurs, such remembered results must be systematically updated.

> **b) Fail-safe defaults:** *Base access decisions on permission rather than exclusion.* … The alternative, in which mechanisms attempt to identify conditions under which access should be refused, presents the wrong psychological base for secure system design. A conservative design must be based on arguments why objects should be accessible, rather than why they should not. **In a large system some objects will be inadequately considered, so a default of lack of permission is safer.** A design or implementation mistake in a mechanism that gives explicit permission tends to fail by refusing permission, a safe situation, since it will be quickly detected. On the other hand, **a design or implementation mistake in a mechanism that explicitly excludes access tends to fail by allowing access, a failure which may go unnoticed in normal use.** This principle applies both to the outward appearance of the protection mechanism and to its underlying implementation.

> **a) Economy of mechanism:** *Keep the design as simple and small as possible.* … design and implementation errors that result in unwanted access paths will not be noticed during normal use (since normal use usually does not include attempts to exercise improper access paths). As a result, techniques such as line-by-line inspection of software … are necessary. For such techniques to be successful, a small and simple design is essential.

> **f) Least privilege:** *Every program and every user of the system should operate using the least set of privileges necessary to complete the job.* … Put another way, **if a mechanism can provide "firewalls," the principle of least privilege provides a rationale for where to install the firewalls.**

### What this supports and what it argues against

**Supports you.** Complete mediation is *about* your problem: "every access to every object." A check that sits at one tool-dispatch function while the same effect is reachable via an embedded agent session and via a Scheme primitive is, by definition, incomplete mediation. Note also the sentence about caching authority results — your embedded agent session that "never consults the policy" is a remembered/assumed authority that was never revalidated. Least privilege's "rationale for where to install the firewalls" is a direct endorsement of choosing the enforcement point by effect.

**Argues against you — two ways, and both bite.**

1. **Fail-safe defaults is the sharpest objection.** Gating *the process-spawning primitive* is a mechanism "that attempts to identify conditions under which access should be refused." Saltzer names the failure mode precisely: such mistakes "tend to fail by allowing access, a failure which may go unnoticed in normal use." Your design is structurally an exclusion list with one entry. The principle says to invert it: enumerate the primitives Scheme may reach at write tier, deny the rest by default.
2. **Economy of mechanism cuts against N enforcement points.** Moving from one check to many scattered checks enlarges the surface that must be inspected line-by-line. This does *not* mean go back to one check at the tool name — it means the *decision logic* must stay in one place even as the *enforcement points* multiply. That is the PEP/PDP split, below.

**Verdict: SUPPORTS the enforcement point, CONTRADICTS the deny-list shape.**

---

## 2. Where real systems put the check

### 2a. Deno — the check is at the op, and Deno explicitly denies your Claim B

From [Deno's security docs](https://docs.deno.com/runtime/fundamentals/security/), verbatim:

> "All code executing on the same thread shares the same privilege level."

> "Code executing in a Deno runtime can use `eval`, `new Function`, or even dynamic import or web workers to execute **arbitrary** code with the same privilege level as the code that called `eval`, `new Function`, or the dynamic import or web worker."

> "It is not possible for different modules to have different privilege levels within the same thread."

This is the closest available analogue to your exact question, and it answers it directly. Deno does *not* attempt to give `eval` a lower privilege than the ops it can reach. It gates the ops (`--allow-run`, `--allow-net`, `--allow-read`) and states plainly that everything on the thread runs at one level. Your instinct to gate the primitive rather than the eval surface matches Deno. But the corollary Deno states, and which your design does not currently honour, is that **the eval tool's tier label is meaningless as a containment claim** — Scheme code obtained via `eval_scheme` runs at whatever the ambient tier is, and can reach every ungated primitive.

**Verdict: SUPPORTS the enforcement point. Explicitly CONTRADICTS the idea that the tier label on `eval_scheme` constrains what evaluated code can do.**

### 2b. Android — the canonical statement that the wrapper is not a boundary

**"Android Permissions Demystified"** — Felt, Chin, Hanna, Song, Wagner, **ACM CCS 2011**, §2.2.1 ([PDF](https://people.eecs.berkeley.edu/~dawnsong/papers/2011%20Android%20permissions%20demystified.pdf)):

> "The API library runs with the same permissions as the application it accompanies, whereas the API implementation in the system process has no restrictions. **The library provides syntactic sugar for interacting with the API implementation.**"

> "**Permission checks are placed in the API implementation in the system process.** … In some cases, the API library may also redundantly check these permissions, but **such checks cannot be relied upon: applications can circumvent them by directly communicating with the system process via the RPC stubs. Permission checks therefore should not occur in the API library.** Instead, the API implementation in the system process should invoke the permission validation mechanism."

And the warning that should worry you most:

> "**There is no centralized policy for checking permissions when an API is called. Rather, mediation is contingent on the correct placement of permission validation calls.**"

Client-side enforcement is never sufficient. But note the second quote: Android pays for effect-level enforcement with *diffuse* mediation, and that paper had to build a tool (Stowaway) and test the API empirically because nobody could tell from the source which entry points were guarded.

**Verdict: SUPPORTS strongly. COMPLICATES via the auditability cost.**

### 2c. Browser extensions — the check is across an IPC hop, in the process holding the capability

**"Protecting Browsers from Extension Vulnerabilities"** — Barth, Felt, Saxena, Boodman, **NDSS 2010** ([PDF](https://www.adambarth.com/papers/2010/barth-felt-saxena-boodman.pdf)), §4.3:

> "if a malicious web site operator manages to corrupt the renderer process (e.g., via a buffer overflow), **the attacker will not be granted access to the extension APIs because the extension core resides in another process.**"

[Chromium Mojo Style Guide](https://chromium.googlesource.com/chromium/src/+/main/docs/security/mojo.md):

> "**When passing objects up a privilege gradient (from less → more privileged), the callee must validate the inputs before acting on them.**"

> "**It is not safe to check for the feature's availability on the renderer side** … Instead, **ensure that the check is done in the process that has power to actually enact the feature. Otherwise, a compromised renderer could opt itself in to the feature!**"

[Chromium: Security Tips for IPC](https://www.chromium.org/Home/chromium-security/education/security-tips-for-ipc/): *"Generally, privileged processes must set all policy."*

Failure evidence, and it is recent: **"Extending a Hand to Attackers: Browser Privilege Escalation Attacks via Extensions"** — Kim & Lee, **USENIX Security 2023** ([PDF](https://www.usenix.org/system/files/usenixsecurity23-kim-young-min.pdf)) found **59 vulnerabilities in 40 extensions**, plus browser-level bugs where the browser process trusted a renderer's *claim* about which extension it was acting for — [crbug.com/1183604](https://crbug.com/1183604): *"Compromised web renderer that \*hasn't\* run any content scripts can spoof chrome.storage (and other api calls) for any extension."* Fixed by `ContentScriptTracker`, i.e. by deriving the identity browser-side instead of accepting the assertion.

**Verdict: SUPPORTS.** Also worth internalising: MAE's MCP surface is your IPC boundary, and an external agent's assertions about its own session tier are the analogue of the renderer's spoofed `MessageSender.id`.

### 2d. Capability systems — enforce at the resource, and abolish ambient authority

**Capsicum** — Watson, Anderson, Laurie, Kennaway, **USENIX Security 2010** ([PDF](https://www.cl.cam.ac.uk/research/security/capsicum/papers/2010usenix-security-capsicum-website.pdf)):

> "a modern web browser must parse HTML, scripting languages, images and video from many untrusted sources, but **because it acts with the full power of the user, has access to all his or her resources (such implicit access is known as ambient authority).**"

Note the shape of Capsicum's actual policy — a pure allow-list:

> "We have constrained `sysctl` by explicitly marking **≈30 of 3000 parameters as permitted** in capability mode; all others are denied."

And §3.1, the enforcement-point passage — the most directly on-topic quote in the paper:

> "**Many system call and capability constraints are applied at the point of implementation of kernel services, rather than by simply filtering system calls.** The advantage of this approach is that a single constraint, such as the blocking of access to the global file system namespace, **can be implemented in one place, `namei`,** which is responsible for processing all path lookups. For example, one might not have expected the `fexecve` call to cause global namespace access, since it takes a file descriptor as its argument rather than a path... However, the file passed by file descriptor specifies its run-time linker via a path embedded in the binary, which the kernel will then open and execute."

> "Similarly, **capability rights are checked by the kernel function `fget`**, which converts a numeric descriptor into a `struct file` reference. We have added a new `rights` argument, allowing callers to declare what capability rights are required to perform the current operation... **Changing the signature of `fget` allows us to use the compiler to detect missed code paths, providing greater assurance that all cases have been handled.**"

> "Capability rights are checked by `fget`... **giving us confidence that no paths exist to access file descriptors without capability checks.**"

The `fexecve` example is your `eval_scheme` example: an argument-surface filter sees a file descriptor and concludes no namespace access occurs, exactly as a tier check on the tool name sees "write" and concludes no process spawning occurs.

**seL4** ([Reference Manual](https://sel4.systems/Info/Docs/seL4-manual-latest.pdf) §2.1; [whitepaper](https://sel4.systems/About/seL4-whitepaper.pdf) §4.1):

> "**Access control governs all kernel services; in order to perform an operation, an application must invoke a capability in its possession that has sufficient access rights for the requested service.**"

> "invoking a capability is **the one and only way** of performing an operation on a system object. In fact, a system call in seL4 is a capability invocation … **The kernel will then check whether the capability authorises the requested operation, and immediately abort the operation if it is not authorised.**"

**Linux Security Modules** — Wright, Cowan, Smalley, Morris, Kroah-Hartman, **USENIX Security 2002** ([PDF](https://www.cs.unibo.it/~renzo/doc/papers/LinuxSecurityModules.pdf)), §3 — *this is the single most on-point passage in the entire review*:

> "**The system call interface** provides an abstraction for userspace to interact with the kernel, and **is a tempting location to mediate access.** … While this is an attractive feature, **mediating the system call interface provides limited value for a general purpose security framework** … This level of mediation is not race-free, may require code duplication, and **may not adequately express the full context needed to make security policy decisions.**"

> "The basic abstraction of the LSM interface is to **mediate access to internal kernel objects** … by placing hooks in the kernel code **just ahead of the access** … the LSM framework has access to the **full kernel context just before the kernel actually performs the requested service.** This improves access control granularity."

And §4.5, the name-is-not-the-object argument at the data-structure level:

> "**The `inode` and `super_block` structures correspond to the actual objects and are independent of names and namespaces.** The `dentry` and `vfsmount` structures … **are associated with a particular name or namespace. Using the first pair of structures avoids object aliasing issues.**"

LSM's designers considered your exact fork — mediate at the named interface, or at the object — and chose the object, for the reason that the interface *lacks context*.

The AppArmor-vs-SELinux debate is the live case study of choosing wrongly. James Morris (LSM co-author), [LKML 2007](https://lkml.iu.edu/hypermail/linux/kernel/0704.2/0318.html): *"**A pathname tells you nothing reliable about the security properties of the object its pointing to.** … think of kernel objects which are protected by locks. **Do you lock the path to the object or do you lock the object itself?**"* Stephen Smalley, [LKML 2007](https://lkml.iu.edu/hypermail/linux/kernel/0706.3/0116.html): *"**The incomplete mediation flows from the design, since the pathname-based mediation doesn't generalize to cover all objects** unlike label- or attribute-based mediation."* Novell's Kurt Garloff [conceded the technical point](https://lkml.iu.edu/hypermail/linux/kernel/0604.2/0560.html) and defended path-based mediation purely on usability — which is the honest shape of the trade-off, and maps onto tool-name dispatch: legible to the operator, unsound as a boundary.

**Verdict: SUPPORTS the enforcement point emphatically, and pre-refutes one of your own proposed counter-arguments (see §4).**

---

## 3. The confused deputy — and why this is where your design actually breaks

Hardy's [original paper](http://cap-lore.com/CapTheory/ConfusedDeputy.html) (*ACM SIGOPS Operating Systems Review* 22(4), 1988, pp. 36–38). The compiler had `(SYSX)STAT` write authority; a user passed `(SYSX)BILL` as the debug-output filename; the billing file was destroyed.

Hardy walks through, and rejects, every check-at-the-deputy fix:

> "Must the compiler check to see if the output file name is in another directory by scanning the file name? No … Should the compiler check for directory name SYSX? No … Should the compiler check for the name (SYSX)BILL? That is not the only sensitive file in SYSX. **Must the compiler be modified whenever new files are added to SYSX?**"

> "**The fundamental problem is that the compiler runs with authority stemming from two sources.** (That's why the compiler is a confused deputy.) … The compiler serves two masters and carries some authority from each … **It has no way to keep them apart.**"

> "Another indication of poor design is that **disparate mechanisms were necessary to arrange separately that the compiler (1) know what file to write on and (2) be authorized to write on that file.**"

**And now the part you need to read carefully.** Tymshare's actual fix was a system call to select which of its two authorities to act under — structurally, *an ambient authority that the privileged operation consults.* Hardy's verdict on that fix:

> "**Note the increase in complexity!** … It soon became clear, however, that **more than two 'authorities' were necessary** for some of our applications. A further problem was that **there were other authority mechanisms besides access to files. Generalizations were not obvious and the modifications to the system were not localized.**"

The seL4 whitepaper restates the diagnosis in exactly the terms that apply to you:

> "**The fundamental problem here is that ACL-based systems use ambient authority for determining access rights.** … **The confusion arises due to ambient authority: The validity of an operation is determined by the security state of the agent (compiler), which in this case is a deputy operating on behalf of an original agent (Alice). For proper security, the access must be determined by Alice's security state.** This means that denomination (the reference to the file) and authority (the right to perform operations on the file) must be coupled, a principle called **no designation without authority.**"

[Miller, Yee, Shapiro, "Capability Myths Demolished", JHU SRL2003-02](https://classpages.cselabs.umn.edu/Fall-2021/csci5271/papers/SRL2003-02.pdf) supplies the precise definition:

> "We will use the term *ambient authority* to describe **authority that is exercised, but not selected, by its user.** In an ambient authority system, subjects are not required to indicate a specific authority in order to exercise it."

> "For example, Unix filesystem permissions constitute an ambient authority mechanism, because **the caller of a function such as `open()` does not choose any credentials to present with the request; the request merely succeeds or fails.**"

> "**When designators and authorities take separate paths through a system, their recombination is likely to lead to confused deputies.**"

> "**In a system where designation and authority are inseparable, this common type of confused deputy problem — in which a malicious party designates a resource they are not supposed to access — simply cannot occur.**"

Mark Miller's dissertation ([*Robust Composition*](https://worrydream.com/refs/Miller_2006_-_Robust_Composition.pdf), §3.2) gives the analogy worth putting in the ADR itself:

> "**With `cp`, you tell it which files to copy by passing it strings.** … In order for `cp` to open the files you name, it must already have the authority to use your namespace, and it must already have the authority to read and write any file you might name. … **The least authority it needs is so broad as to make achieving either security or reliability hopeless.**"
> "**With `cat`, you tell it which files to copy by passing it the desired (read or write) access to those two specific files.** … Its least authority is what you'd expect."

Same task, same enforcement depth, catastrophically different least authority — the variable is whether authority travels with the request.

### Is your situation an instance? Yes, textbook.

The AI agent holds the user's authority. It is driven by content the user did not author — cloned repos, fetched pages, shared KBs. [Willison's lethal trifecta](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/) names the preconditions (private data + untrusted content + external communication) and the reason no in-model fix works: *"LLMs are unable to reliably distinguish the importance of instructions based on where they came from."*

**What the literature says about where the check must live in this situation:** not merely "at the effect" but **on authority carried with the request**, not read from session state. [CaMeL, "Defeating Prompt Injections by Design"](https://arxiv.org/pdf/2503.18813) (Debenedetti, Shumailov, Fan, Hayes, Carlini, Fabian, Kern, Shi, Terzis, Tramèr — Google/DeepMind/ETH, 2025) is the concrete instantiation: a custom interpreter that "**tracks provenance and enforces security policies**," with capabilities as per-value tags recording "the value's sources and allowed readers," and policies evaluated *at the tool call* against those tags. Untrusted data "can never impact the program flow."

**This is the finding that should change your ADR.** Your process-spawn primitive checking the *ambient tier* is the Tymshare "switch hats" patch — the fix Hardy documented as not generalising. It answers "is this session allowed to spawn?" It cannot answer "did the argument to this spawn originate from the user or from a cloned repo's README?" Those are different questions, and only the second one is the confused-deputy question. Your spawn primitive "does not choose any credentials to present; the request merely succeeds or fails" against session state — Miller/Yee/Shapiro's definition of ambient authority, verbatim.

**Verdict: SUPPORTS enforcing at the effect. CONTRADICTS enforcing against an *ambient* tier. The check is in the right place, consulting the wrong thing.**

---

## 4. The strongest case against — including which of your candidate counter-arguments don't survive

You proposed three. Two are real; one is backwards.

### ✗ "Checking deep in the stack lacks the context to decide correctly" — mostly false here

LSM's designers concluded the opposite: the *interface* lacks context; the object-level hook has "the full kernel context just before the kernel actually performs the requested service." Garfinkel's §5.4 is titled ["Let the Kernel Do the Work"](https://cs155.stanford.edu/papers/traps.pdf) and argues for pushing checks deeper, not shallower: *"If the kernel does some complex operation, don't try to replicate that code yourself, just call the code in the kernel."* His §6 goes further, abandoning the syscall boundary entirely and pointing at LSM: *"Another important question to be addressed is whether the system call boundary remains best place to interpose on applications access to sensitive resources at all."*

The one place this argument *is* true is [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html): *"BPF programs may not dereference pointers which constrains all filters to solely evaluating the system call arguments directly."* That is a genuine "too deep to see the path" case — but it's an artifact of a deliberate TOCTOU-avoidance constraint, not a general property of low-level checks. In an in-process Rust primitive you have full argument access. **Drop this counter-argument; it doesn't apply to you.**

### ✓ Late refusal produces bad errors *and* partial side effects — real, and Garfinkel documents it

**"Traps and Pitfalls: Practical Problems in System Call Interposition Based Security Tools"** — Garfinkel, **NDSS 2003**, §4.5 *Side Effects of Denying System Calls*:

> "Preventing the execution of a system call, or causing a system call to return in a manner inconsistent with its normal semantics, **can have a detrimental impact on the operation of the application, potentially undermining its reliability and even its security.**"

> "**Denying calls that an application uses to drop privilege frequently introduces serious security flaws.** … Many applications that rely on `setuid` fail to check its return value, and if `setuid` fails, will continue to function in a compromised state. Upon casual examination we were able to discover this condition in several common FreeBSD daemons, and it appears that this problem is quite widespread."

> "Given that aborting privilege-dropping calls will often undermine the security model of a sandboxed application, it seems generally advisable to allow all such calls. For `setuid` and related calls, **it seems most prudent to abort the application entirely if we wish to deny a call.**"

Lesson from §5.5: *"Any time you change the behavior of your operating system, for example by aborting system calls, **you risk breaking your applications and potentially introducing new security holes.** Avoid making changes that conflict with normally specified OS semantics, or diverge from application designer's expectations."*

Reinforced by [Google AIP-211](https://google.aip.dev/211): *"Services **must** check authorization before validating any request, to ensure both a secure API surface and a consistent user experience."* — i.e. Google's normative API guidance is explicitly *check early*.

**Applies to you directly.** A Scheme program that has already mutated buffers, written files, and updated the KB, and *then* gets denied at `(spawn ...)`, leaves MAE in a half-applied state with no rollback. If the Scheme code doesn't check the return value of the failed primitive and keeps going, Garfinkel's `setuid` scenario reproduces exactly. Garfinkel's own recommendation — abort the whole evaluation rather than return an error the caller may ignore — is the mitigation, and it should be in the ADR.

### ✓ A deny-list is only correct while complete — real, and this is the fatal one

[JEP 411, "Deprecate the Security Manager for Removal"](https://openjdk.org/jeps/411) is the most valuable postmortem available, because Java tried precisely your architecture — effect-level checks scattered through a large library, consulting ambient stack authority — for 25 years:

> "The small size of the Java class libraries — only eight `java.*` packages in Java 1.0 — **made it feasible** for code in, e.g., `java.io` to consult with the Security Manager before performing any operation."

> "the rapid growth of `java.*` and `javax.*` packages led to **dozens of permissions and hundreds of permission checks throughout the JDK. This is a significant surface area to keep secure, especially since permissions can interact in surprising ways. Some permissions … allow application or library code to perform a series of safe operations whose overall effect is sufficiently unsafe that it would require a more powerful permission if granted directly.**"

> "it is an ongoing maintenance burden. **All new features and APIs must be evaluated to ensure that they behave correctly when the Security Manager is enabled.**"

> "**There is no way to have partial security, where only a few resources are subject to access control.**"

> "**Difficult programming model** — The Security Manager approves a security-sensitive operation by checking the permissions of all running code that led up to the operation. … The path of least resistance for application developers is often to grant `AllPermission` to any relevant JAR file, but this again runs counter to the principle of least privilege."

Microsoft reached the same conclusion. From [Microsoft Learn on Code Access Security](https://learn.microsoft.com/en-us/previous-versions/dotnet/framework/code-access-security/code-access-security):

> "**CAS in .NET Framework should not be used as a mechanism for enforcing security boundaries based on code origination or other identity aspects. CAS and Security-Transparent Code are not supported as a security boundary with partially trusted code, especially code of unknown origin.** … **.NET Framework will not issue security patches for any elevation-of-privilege exploits that might be discovered against the CAS sandbox.**"

The [obsoletion note](https://learn.microsoft.com/en-us/dotnet/core/compatibility/core-libraries/5.0/code-access-security-apis-obsolete) adds the failure mode: APIs carried forward but inert *"led to 'fail open' scenarios, where some CAS-related APIs exist and are callable but perform no action at runtime."*

Both vendors independently concluded that a large runtime with per-effect checks against ambient authority cannot be kept complete. Note that CAS is the *closest structural analogue to your design in the entire review*: in-process, same-runtime, checks at the resource-touching API, consulting ambient call-stack authority. It is the design you have, at industrial scale, abandoned.

Two further completeness costs, empirically attested:

- **Android**: *"There is no centralized policy for checking permissions when an API is called. Rather, mediation is contingent on the correct placement of permission validation calls."* (Felt et al., CCS 2011) — researchers had to build a static-analysis tool and test empirically because the guarded set was not knowable from the source.
- **Garfinkel §4.2, "Overlooking Indirect Paths to Resources"**: *"One of the key difficulties of interposing on an interface as complex as the Unix API is simply knowing all of the side effects and non-obvious ways that one can affect system resources. **It is important to identify every possible way for a process to access or modify resources, both alone and working in concert with other processes.**"*

**Verdict on §4: your first two candidate counter-arguments are real; the third is backwards. The decisive one is completeness, and it is not merely theoretical — it has two vendor retreats behind it.**

---

## 5. Interpreters as a privilege-escalation vector — the crux

**Plainly: no. The consensus is that you cannot safely expose an interpreter at a lower privilege than its most dangerous primitive by gating that primitive.** No mature system claims otherwise; the ones that tried have retreated.

**Node.js** ([vm docs](https://nodejs.org/api/vm.html)): *"**The `node:vm` module is not a security mechanism. Do not use it to run untrusted code.**"*

**Deno** (quoted in §2a): eval runs "with the same privilege level as the code that called `eval`," and "It is not possible for different modules to have different privilege levels within the same thread." This is a runtime whose *entire product identity* is a permission system, stating that intra-runtime privilege separation is not on offer.

**PostgreSQL** is the closest structural analogue to MAE — an embedded interpreter offered at two trust levels. From [Trusted and Untrusted PL/Perl](https://www.postgresql.org/docs/current/plperl-trusted.html):

> "In general, the operations that are restricted are those that interact with the environment. This includes file handle operations, `require`, and `use` (for external modules)."

> "**Trusted PL/Perl relies on the Perl `Opcode` module to preserve security. Perl documents that the module is not effective for the trusted PL/Perl use case. If your security needs are incompatible with the uncertainty in that warning, consider executing `REVOKE USAGE ON LANGUAGE plperl FROM PUBLIC`.**"

> "While PL/Perl functions run in a separate Perl interpreter for each SQL role, all PL/PerlU functions executed in a given session run in a **single Perl interpreter (which is not any of the ones used for PL/Perl functions)**. … **no communication can occur between PL/Perl and PL/PerlU functions.**"

Read those three together. PostgreSQL does *three* things you do not: (1) restricts by **category of capability** (everything that "interacts with the environment"), not by one primitive; (2) runs privileged and unprivileged code in **separate interpreter instances** so state cannot leak between them; and (3) *still* ships an explicit warning that the mechanism may not hold, with a documented escape hatch to revoke the language entirely. That is what a serious attempt at your design looks like, and its own maintainers hedge it.

**Java** — JEP 411, quoted in §4. The line *"Some permissions … allow application or library code to perform a series of safe operations whose overall effect is sufficiently unsafe that it would require a more powerful permission if granted directly"* is the general statement of why gating one primitive fails: reachability in an expressive interpreter is transitive and compositional. Gate `spawn`, and a sequence of individually-permitted write-tier operations composes into the same effect (write a file into a watched directory; register a hook; trigger a formatter; edit a `.git/hooks` script; modify `Makefile` and invoke `run_build`).

**.NET CAS** — see §4. Same architecture, same outcome, plus the explicit refusal to issue security patches for sandbox escapes.

**MAE-specific hazard nobody else has to worry about.** CLAUDE.md principle #6 states runtime redefinability is *sacred* — "Users must be able to redefine any function while the editor is running." In a single shared Scheme image, that is an escalation primitive: write-tier Scheme can redefine a function that privileged Scheme later calls. This is Garfinkel's "indirect paths to resources" applied to a mutable global environment, and it is strictly worse than the Perl case PostgreSQL felt obliged to solve with separate interpreters. **Redefinability and single-image eval-at-a-lower-tier are in direct tension, and the ADR must say which one wins.**

What the systems that *succeed* do instead — uniformly an allow-list plus removal of ambient authority:

- **Capsicum**: ≈30 of 3000 `sysctl` parameters permitted, all others denied; global namespaces removed entirely rather than filtered.
- **PostgreSQL trusted PL**: whole categories of capability removed from the interpreter, plus interpreter separation.
- **CaMeL**: a purpose-built interpreter that tracks provenance per value and evaluates policy at the tool call.
- **seL4**: no global namespace exists at all; the only way to act is to present a capability.

**Verdict: CONTRADICTS.** This directly invalidates keeping `eval_scheme` at write tier on the theory that gating spawn contains it. It does *not* invalidate gating spawn — that check should stay. It invalidates the claim that the check is *sufficient*.

---

## Recommendations

Keep the enforcement point. Change three things.

**1. Keep effect-level PEPs; centralise the PDP.**
Per [NIST SP 800-162](https://nvlpubs.nist.gov/nistpubs/specialpublications/NIST.SP.800-162.pdf) §: a **Policy Decision Point** *"Computes access decisions by evaluating the applicable DPs and MPs"*; a **Policy Enforcement Point** *"Enforces policy decisions in response to a request from a subject requesting access to a protected object; the access control decisions are made by the PDP."* NIST notes *"PDP and PEP functionality can be distributed or centralized, and may be physically and logically separated from each other."*
Many PEPs at effects, **one** PDP holding the tier logic. This satisfies complete mediation *and* economy of mechanism (§1's two competing pulls), and it answers the reviewer question "why is this scattered now?" — it isn't; only enforcement is distributed, the decision is not.

**2. Make it an allow-list, fail-closed — and let the compiler prove completeness.**
Classify every Scheme primitive by required tier. Do **not** document this as a convention. Follow Capsicum: change the signature of the single function every primitive invocation must pass through so that it *takes* a required tier/rights argument — *"Changing the signature of `fget` allows us to use the compiler to detect missed code paths, providing greater assurance that all cases have been handled."* In Rust this is cheap, and it converts JEP 411's "ongoing maintenance burden" into a compile error. An unclassified primitive must fail to compile, or default to `privileged`.
First task under this: find MAE's `namei`/`fget` — the one function through which all primitive dispatch converges. If no such chokepoint exists, creating one is the highest-value refactor in this whole ADR, because the completeness claim is only available when every path converges.

**3. Replace ambient tier with carried authority.**
This is the confused-deputy fix and the only one that addresses your actual stated threat model (agent driven by content the user did not author). The tier consulted at the effect should come from the *request*, tagged with provenance: did this call chain originate from a user keystroke, or from text in a fetched page / cloned repo / shared KB? CaMeL is the worked example. Minimum viable version: an explicit authority token threaded through dispatch, with `with_ai_dispatch_scope` (which MAE already has) tagging AI-originated work as content-derived, and effects failing closed when the tag is content-derived rather than user-derived.

**4. Decide partial-application semantics explicitly.**
Garfinkel §4.5 says silent failure-and-continue is the dangerous option. Adopt his recommendation: abort the entire Scheme evaluation on a tier denial rather than returning an error the calling Scheme code may ignore. State this in the ADR, because it is a user-visible behaviour change.

**5. Separate the interpreter instances.**
PostgreSQL's separate-interpreters design is the single most transferable idea in §5. If privileged and write-tier Scheme share one mutable image, principle #6 (sacred runtime redefinability) makes that image an escalation channel. If full separation is too costly now, say so in the ADR as accepted, tracked debt with an `@ai-caution: [architecture-debt]` marker — do not leave it unstated.

**6. State plainly that `eval_scheme`'s tier is a *calling* requirement, not a containment claim.**
Otherwise a future reader — human or AI — will assume the label bounds what evaluated code can do. Deno's docs are the model for how bluntly to write this.

---

## Confidence and gaps

Everything quoted above I retrieved and verified against primary text (papers extracted locally from PDF; docs fetched directly). Caveats:

- I did **not** verify a catalogue of Deno permission-bypass CVEs where a check was missing from one op but present in siblings. That would strengthen §4's completeness argument but is not load-bearing — JEP 411 and the .NET CAS retreat already carry it.
- Three background threads (Deno CVE history, Android enforcement-inconsistency papers such as Kratos/ACMiner, and further interpreter-sandbox material including vm2's CVE series and RestrictedPython) had not returned at time of writing. Their content is substantially covered by sources verified directly. The one genuine gap is empirical *counts* of enforcement-gap bugs in Android's middleware — the strongest available quantitative case for the auditability cost of effect-level enforcement.

**Two citation corrections**, since both errors are commonly made:

- **Klein et al., SOSP 2009** (the seL4 verification paper) does **not** contain a "the kernel checks the capability at invocation" statement. Cite the seL4 Reference Manual §2.1 or the whitepaper §4.1.
- **Miller's 2006 dissertation** contains no confused-deputy exposition beyond a one-sentence attribution in §3.5, and never uses the phrase "no designation without authority" — that wording is *Capability Myths Demolished*'s. Route confused-deputy citations to Hardy 1988 or Miller/Yee/Shapiro 2003.
