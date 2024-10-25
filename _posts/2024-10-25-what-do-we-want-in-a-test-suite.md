---
layout: post
title: "What do we want in a test suite?"
categories: [opinion]
tags: [testing]
permalink: "/blog/2024/10/25/what-do-we-want-in-a-test-suite/"
has_mermaid: false
---

I remember the "dark ages" of software development, when automated testing was still an exotic idea.
One had to be careful not to change too much, lest it all break and come crashing down. One was
_afraid_ to touch the software. Refactoring was taboo. "Don't touch a running system" and "if it
ain't broke, don't fix it" were the slogans of the day. The software would inevitably rot into an
unmaintainable mess. Any changes at all would be prohibitively expensive, so one would change as
little as possible.

<!--more-->

Nowadays we know better. We're supposed to write tests which verify the software automatically. The
promise: when all the tests pass, you can release. No need to be afraid! Make any change you feel
necessary: refactor the code to clean up some past mess or so that it accommodates a new business
requirement just right. If you broke something, the tests will catch the problem before it causes
real damage.

Only it doesn't always work that way. The tests themselves start to get in the way. They run slowly.
They fail sporadically. They interfere with attempts to refactor the code. When they do fail, fixing
them becomes a days-long, unplanned endeavour. Sometimes bugs -- including regressions -- even make
it through the tests. We lose the confidence to push on green. That fear comes back.

In short: just having some tests alone is not enough. Done poorly, they can cost time and nerves
while not really delivering confidence.

But _done right_, automated testing truly revolutionizes software development. So what do we mean by
that?

## The properties of good test suites

There already exist plenty of resources to explain what one should and shouldn't do when writing
automated tests. I don't intend to repeat them here. Instead, I'd like to consider what properties a
test suite should have, starting from first principles.

<div class="note">
  <h5>
    <span class="icon">
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 20h9v2h-9zM22 2l-2 2-4-4 2-2zM3 17v4h4l10-10-4-4L3 17z"/>
      </svg>
    </span>
    Note
  </h5>
  <p>
    I'm going to focus on the test suite itself here, taking it as an existing artefact. This
    isn't about how easy it is to write <i>new</i> tests.
  </p>
</div>

The first desired property should be obvious:

> (1) When production is broken, at least one test fails.

This is, after all, _the very purpose of the test suite._

Now, one could achieve this property with a single test: compare the source code of the application
being built with a fixed "golden" copy of the source code. Fail the test if they differ. The problem
is that this prohibits _any_ changes to the code, even completely harmless ones! We want the
_freedom_ to change the code _without fear of delivering broken software_. So we need the next
property as a counterpoint:

> (2) When production is not broken, no test fails.

Already with these two properties, the test suite becomes interesting. It has to distinguish between
working and broken software. It must _encode knowledge_ of what it means for the software to be
correct.

Of course, tests do fail, and when they do, we need to know whether the production code is broken or
it's a problem with the test itself. We don't want to lose the whole day figuring this out. Anyone
who has had to debug a slow end-to-end test blocking a critical release will appreciate this. Thus
our next property:

> (3) When a test fails, it's easy to diagnose why.

One well-known rule of software development is that the cost of a defect increases rapidly (some
would say exponentially) with the time between its introduction and its discovery. A typo leading to
a broken build incurs almost no cost if caught directly by the IDE. A bug caught only by an
end-to-end test running on a CI pipeline incurs much more work to diagnose and fix. And a defect
which reaches production could cause real loss to the business. Even more so if the defect isn't
discovered and reported immediately.

In short: _early feedback is good._ How do we get that? Run the tests _often_. Ideally after every
single change. But who will wait hours -- or even minutes -- for all the tests to run after every
change to the code? Hardly any developer will do so. Even if they did, the cost they would impose
would be pretty high. Hence our next property:

> (4) The tests run quickly.

The final property pertains to one of the most pernicious factors which drive up the cost of
automated test suites: flakiness.

> (5) Each test failure is reliably reproducible, not a result of random chance.

Property (1) is about the _completeness_ of the test suite, while the remaining properties are about
minimizing the _cost_ of the test suite. So there is some natural tension between them as we discuss
next.

## For which properties are we optimizing?

Consider the replacement of real components in the system under test with test doubles. This serves
properties (4) (in case the test double replaces a slow external component) and (5) (in case the
external component is also unreliable). It can also serve property (3): a bug in the replaced
component will not cause a failure in the test. This can reduce the surface area to be investigated.

At the same time, the use of test doubles can complicate properties (1) and (2). How does one know
that the test double behaves the same way the real component would? If that is not guaranteed, then
differences between the behaviour assumed by the test and in the real component are likely points
for defect to appear.

And what does property (2) imply? For one thing, a refactoring (by definition) does not cause
production to break, so should not cause any test to fail. If the refactoring affects the way two
components interact, and a test replaces one of those with a test double, then that test will likely
have to be updated. So the test is broken even though production is not.

So, properties (1) and (2) nudge towards using real components rather than test doubles. But
properties (3), (4), and (5) suggest the opposite. Going too far in either direction will yield poor
results. A test suite in which every collaborator is replaced by a test double is a nightmare to
maintain. It's also likely to introduce major gaps in coverage as the behaviour of the test doubles
begins to diverge from the real application. But a test suite which _never_ uses test doubles will
be slow, flaky, and expensive to maintain, assuming it's even feasible to build.

## Measuring success

One can imagine ways to measure the properties above:

- For property (1): How often do defects escape the test suite and make it into production?
- For property (2): How often does it occur that a test failure is caused by a defect in the test
  rather than the production code?
- For property (3): How long does it take to diagnose and repair a failing test?
- For property (4): How long does it take the entire test suite to run?
- For property (5): How many tests fail sporadically, and how often?

Suppose we construct concrete concrete _metrics_ out of the above questions. We might consider these
five metrics components of a "test suite health score". One could put them on a dashboard somewhere
and incentive developers to improve them.

Most importantly, metrics provides a target which can guide conversations and justify investments.
If you can show that some architectural refactoring pushes a metric in the right direction, then you
have a basis for comparing costs with benefits. An argument in terms of developer ergonomics is
likely to fall on deaf ears with non-developers. One based on costs and benefits is more likely to
find an audience.

However, as we saw above, focusing too much on a single metric to the detriment of the others won't
yield good results. Much like the
[Four Key Metrics of the book _Accelerate_](https://itrevolution.com/product/accelerate/), one
should optimize for _all_ metrics.

## Conclusion

in this piece I attempted to define the properties a good test suite should have. I showed how these
properties interact and are, to some degree, in tension with each other. I then proposed to measure
them to form a kind of health score for test suites.

Can you think of any other properties a good test suite should have?
[Reach out!](https://hovinen.tech/)
