---
layout: post
title: "Introducing Test That!"
categories: [announcements]
tags: [rust, testing]
has_mermaid: false
---

I'm excited to announce the release of [Test That!](https://crates.io/crates/test-that), a powerful
library for test assertions in Rust. It is a fork of [GoogleTest Rust].
<!--more-->

## TL;DR

Test That! allows you to write test assertions which precisely specify your _intent_:

```rust
let vec = vec![5, 123, -4];
verify_that!(vec, each(gt(0)))
```

and get informative, meaningful diagnostics when the tests fail:

```
Value of: vec
Expected: only contains elements that is greater than 0
Actual: [5, 123, -4],
  whose element #2 is -4, which is less than or equal to 0
```

## What's new about Test That!?

Compared with GoogleTest, Test That! offers some improvements:

- It has a cleaner, simpler, easier to use API than recent versions of GoogleTest. I explain this
  in more detail [below](#why-did-i-fork-googletest).
- Compared with older versions of GoogleTest, it removes various limitations in composability of
  the matchers; see [below](#solving-the-limitations-of-googletest-011).
- All dependencies of Test That! are optional. So turning off all features results in the crate
  having no dependencies at all. This means that capabilities requiring dependencies, including
  non-fatal assertions, regexes, and floating point matchers are then not available. The default
  feature set includes all of these, however.
- I have extended the shorthand syntax for matching against containers in the [`verify_that!`] macro
  and friends to other macro-based matchers, such as [`matches_pattern!`]. So one can write things
  like:

  ```rust
  matches_pattern!(MyStruct { a_vec: [eq(1), eq(2), eq(3)] })
  ```
- The type alias `Result` is now called [`TestResult`], so that it does not collide with other
  `Result` types one might want to use.
- I've renamed `unordered_elements_are!` to [`contains_exactly!`] and added a method [`in_order()`]
  to that matcher. So instead of `elements_are!`, one now uses `contains_exactly![...].in_order()`.
  The existing structure had always bothered me: it was inherited from the [GoogleTest C++ library],
  where it had grown over time. I wanted a clean break, and this structure feels more natural: the
  stronger constraint requires more syntax than the weaker one.
- The "subset" [`contains_each!`] and "superset" [`is_contained_in!`] matchers now support enforcing
  that the elements be in the same order as their corresponding matchers using the method
  [`in_order()`].
- The syntax for matching against `HashMap` in `contains_exactly!` now represents key-value pairs
  with an arrow operator `=>`:

  ```rust
  let value = HashMap::from([(1, "one"), (2, "two"), (3, "three")]);
  verify_that!(value, contains_exactly![eq(1) => eq("one"), eq(2) => eq("two"), eq(3) => eq("three")])
  ```
  Previously, one represented them with pairs, which meant that one could not match against a `Vec`
  of pairs with that matcher.
- I have split the [`Matcher`] trait so that the [`describe()`] method is in a new trait called
  [`Describable`]. This allows a reduction in code duplication in certain cases.

For anyone interested in porting from GoogleTest to Test That!: Don't worry! I've included some
[features](https://github.com/hovinen/test-that#porting-from-googletest-rust) which add aliases to
make it easier to port existing code.

## Why did I fork GoogleTest?

I spearheaded the GoogleTest crate a few years ago when I worked at Google. The goal was to bring
the power assertions of the GoogleTest C++ library to Rust. I was involved in GoogleTest until
shortly after I left in 2023. Version 0.12 introduced a
[critical change](https://github.com/google/googletest-rust/pull/367) which substantially affected
the design assumptions of the library. I hold that this change dramatically worsened developer
experience. To see how, let's go through some examples.

Let's start with a simple data model:

```rust
#[derive(Debug)]
struct AStruct {
    value: u32,
    string: String,
}
```

Suppose I have a value of this struct:

```rust
let value = AStruct {
    value: 123,
    string: "Hello, world!".into(),
};
```

Now suppose I want to assert that the data in the struct are what I expect them to be. In GoogleTest
0.11, this would appear as follows:

```rust
verify_that!(
    value,
    matches_pattern!(AStruct {
        value: eq(123),
        string: eq("Hello, world!"),
    })
)
```

If I try the same thing with GoogleTest 0.12 and later, I get some errors:

```
error[E0277]: can't compare `&u32` with `{integer}`
    --> src/main.rs:25:13
     |
  23 | /          verify_that!(
  24 | |              value,
  25 | |/             matches_pattern!(AStruct {
  26 | ||                 value: eq(123),
  27 | ||                 string: eq("Hello, world!"),
  28 | ||             })
     | ||______________^ no implementation for `&u32 == {integer}`
  29 | |          )
     | |__________- required by a bound introduced by this call
     |
```

Wait, what? It's talking about comparing something with a reference. But there are no references
anywhere to be seen in my code.

Okay, then I'll take a reference in front of the number:

```rust
verify_that!(
    value,
    matches_pattern!(AStruct {
        value: eq(&123),
        string: eq("Hello, world!"),
    })
)
```

All right, then I guess I need to take references in front of numbers in GoogleTest now. Let's now
try applying this knowledge elsewhere:

```rust
let value = 123;
verify_that!(value, eq(&123))
```

When we compile now, we get:

```
error[E0277]: can't compare `{integer}` with `&{integer}`
    --> src/main.rs:36:29
     |
  36 |         verify_that!(value, eq(&123))
     |         --------------------^^^^^^^^-
     |         |                   |
     |         |                   no implementation for `{integer} == &{integer}`
     |         required by a bound introduced by this call
     |
     = help: the trait `PartialEq<&{integer}>` is not implemented for `{integer}`
     = help: the following other types implement trait `PartialEq<Rhs>`:
               f128
               f16
               f32
               f64
               i128
               i16
               i32
               i64
             and 8 others
     = note: required for `EqMatcher<&{integer}>` to implement `googletest::matcher::Matcher<{integer}>`
```

Okay, so let's get rid of the reference and see what happens:

```rust
let value = 123;
verify_that!(value, eq(123))
```

That compiles. Turns out that in _that_ context, one _can't_ take a reference, while in the
_previous_ context, one _must_.

This is becoming confusing.

Let's try something else. Suppose I have a newtype as follows:

```rust
#[derive(Debug)]
struct NewType(u32);
```

I might have previously matched against it as follows:

```rust
let value = NewType(123);
verify_that!(value, matches_pattern!(NewType(eq(123))))
```

Nowadays, it seems I must do the following:

```rust
let value = NewType(123);
verify_that!(value, matches_pattern!(NewType(eq(&123))))
```

This compiles and runs fine. But suppose I now change `NewType` to make it `Copy`:

```rust
#[derive(Debug, Clone, Copy)]
struct NewType(u32);
```

Suddenly, my test no longer compiles!

```
error[E0277]: can't compare `u32` with `&{integer}`
    --> src/main.rs:46:29
     |
  46 |         verify_that!(value, matches_pattern!(NewType(eq(&123))))
     |         --------------------^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^-
     |         |                   |
     |         |                   no implementation for `u32 == &{integer}`
     |         required by a bound introduced by this call
     |
help: the trait `PartialEq<&{integer}>` is not implemented for `u32`
      but trait `PartialEq<u32>` is implemented for it
    --> /home/hovinen/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/cmp.rs:1875:13
     |
1875 |               impl const PartialEq for $t {
     |               ^^^^^^^^^^^^^^^^^^^^^^^^^^^
...
1897 | /     partial_eq_impl! {
1898 | |         bool char usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 f16 f32 f64 f128
1899 | |     }
     | |_____- in this macro invocation
     = help: for that trait implementation, expected `u32`, found `&{integer}`
     = note: required for `EqMatcher<&{integer}>` to implement `googletest::matcher::Matcher<u32>`
     = note: 3 redundant requirements hidden
     = note: required for `CompileAssertAndMatch<NewType, IsMatcher<'_, ...>>` to implement `googletest::matcher::Matcher<NewType>`
```

This is very strange indeed. Adding `Copy` to an existing struct should be a compatible change --
one is only _adding_ a capability (and constraint) to the type, after all. Downstream users of the
type shouldn't break because of it.

These are only a few simple examples. It becomes more confusing the more one tries to do. (Don't get
me started about the `ref` keyword!) Multiply this by a huge code base with deep, complex data
structures and complex assertions on them, and this quickly turns into a huge mess.

One driving idea behind GoogleTest was that writing unit tests should be effortless. One shouldn't
have to devote much mental bandwidth to writing useful assertions. One shouldn't have to think, "How
do I get the test to assert just this one property _and_ make sure the assertion failure message is
meaningful?" One just lets the matchers take care of it.

I'm sure that I could find the right mental model to navigate this if I sat down and thought about
it long enough. But that takes _serious_ mental bandwidth, which there must be a _very_ good
justification for investing. Especially since I must not only invest it myself, but also convince my
colleagues and collaborators to do the same.

Why was this change introduced? There were in fact some limitations in the existing design of
GoogleTest. It worked fine probably 99% of the time, but for certain corner cases, it failed pretty
badly. Below, I show some of those restrictions and how I was able to remove them _without_
incurring such a high price.

## Solving the limitations of GoogleTest 0.11

I decided not to port my own projects but to stick with GoogleTest 0.11. However, with time, the
various warts and limitations of GoogleTest 0.11 were becoming hard to ignore. And, given that
upgrading was not an option for me, I was working literally with an orphaned library, with no chance
that any of those problems could ever be fixed upstream.

What kinds of problems do I mean?

Well, first, there were some annoying limitations in what one could do with the matchers. Take the
`matches_pattern!` macro which appears in the examples above. One could use it to match against the
return values of methods:

```rust
impl AStruct {
    fn get_value(&self) -> u32 {
        self.value
    }
}

verify_that!(value, matches_pattern!(AStruct {
    get_value(): eq(123),
}))
```

But this didn't work when the return value was a slice or a string slice:

```rust
impl AStruct {
    fn get_string(&self) -> &str {
        &self.string
    }
}

verify_that!(value, matches_pattern!(AStruct {
    get_string(): eq("Hello, world!"),  // Compiler error!
}))
```

I found I could not match on methods returning `Option<&SomeType>`, which hampered assertions on the
sources of [anyhow](https://crates.rs/crates/anyhow) errors.

It [stumbled](https://github.com/google/googletest-rust/issues/323) with methods which narrowed the
lifetime of an internally held reference. And it didn't
[support](https://github.com/google/googletest-rust/issues/351) containers which produced owned
rather than borrowed values.

Over time, these limitations began to add up and cause some real headaches. So I began investigating
how to address these limitations _without_ changing the fundamental structure of the library.

### Two design flaws

There are two interacting design flaws in GoogleTest 0.11. By fixing them, I was able to resolve all
of the limitations mentioned above.

First, I originally wanted to keep the API surface as small as possible. This meant that most
matcher functions would return opaque `Matcher` implementations, while the concrete types would
remain private to the library. A (simplified) matcher might look roughly like this:

```rust
pub fn eq<T>(value: T) -> impl Matcher {
    EqMatcher { value }
}

struct EqMatcher<T> {
    value: T,
}
```

The `Matcher` trait itself needs to know against what types it can match. There are two ways of
doing this: as a type parameter to `Matcher` or as an associated type of this trait. After initially
settling on a type parameter, and being
[bitten](https://hovinen.me/blog/2023/06/06/demystifying-trait-generics-in-rust/) by some problems
with type resolutions, I settled on using an associated type.

```rust
impl<T> Matcher for EqMatcher<T> {
    type ActualT = ???

    fn matches(&self, actual: &Self::ActualT) -> MatcherResult {...}
}
```

Now one _could_ just put `T` there, but this makes the matcher a little too rigid. For example, it's
perfectly valid to compare equality of a string slice and an owned `String`:

```rust
let slice = "Hello, world";
let string = String::from("Hello, world");
assert!(slice == string);
```

So the actual and expected types don't have to be the same. They only have to be comparable, which
is expressed through the [`PartialEq`] trait.

To support this, one needs _another_ type parameter representing the type against which one is
matching.

```rust
impl<ExpectedT, ActualT: PartialEq<ExpectedT>> Matcher for EqMatcher<ExpectedT> {
    type ActualT = ActualT

    ...
}
```

But this won't compile, since the type `ActualT` in the impl block is _unconstrained_. It applies to
_any_ type `ActualT` which can be compared with the type of the expected value. But there can be
only one implementation for any fixed `ExpectedT`. So the compiler doesn't know which one to pick.

To fix this, I had to make `ActualT` a type parameter of the struct and the matcher function.

```rust
pub fn eq<ExpectedT, ActualT>(value: ExpectedT) -> impl Matcher {
    EqMatcher { value, PhantomData }
}

struct EqMatcher<ExpectedT, ActualT> {
    value: ExpectedT,
    phantom: PhantomData<ActualT>,
}
```

Here's the problem: _This means that the type `ActualT` is fixed by the call site of `eq`_. Suppose
now that `ActualT` is a reference with a lifetime. That lifetime is part of the type. So the
_lifetime_ is now fixed. But there are certain matchers which extract the actual value via a
closure. This is how property values are obtained in [`matches_pattern!`], for example. In such
cases, the lifetime cannot be known at the call site. The code acts conceptually like this:

```rust
let matcher = eq(expected);
let closure = |s: &MyStruct| s.get_value();
matcher.matches(closure(actual));
```

If `MyStruct::get_value()` returns an owned value with a `'static` lifetime, there's no problem.
But if it returns a value bound to the lifetime of the closure's parameter, then the type of
`matcher` is too rigid. The type against which it matches must satisfy a lifetime bound _for every
lifetime_ rather than for a _fixed_ lifetime.

Test That! solves this with three key changes:

- First, the `Matcher` trait now takes the actual value as a type parameter again. This allows it
  to be instantiated freely for multiple types. In particular, the lifetime of the actual type no
  longer has to be fixed.
- Second, the various matchers and their creation functions are no longer parameterized by the type
  against which they match. These types are instead determined by where they are _used_.
- Finally, to make this all work, I gave up on keeping the matcher structs out of the public API
  surface. The matcher functions now just return the concrete structs rather than opaque `Matcher`
  impls.

The result looks approximately like this:

```rust
pub fn eq<ExpectedT>(value: ExpectedT) -> EqMatcher<ExpectedT> {
    EqMatcher { value }
}

struct EqMatcher<ExpectedT> {
    value: ExpectedT,
}

impl<ActualT, ExpectedT> Matcher<ActualT> for EqMatcher<ExpectedT> {
    fn matches(&self, actual: &ActualT) -> MatcherResult {...}
}
```

This structure resolves the known limitations of older versions of GoogleTest. Methods returning
slices and string slices now work seamlessly with `matches_pattern!`. As do methods returning
structures containing references, in various combinations. With a few more tricks, I was also able
to add support for containers which iterate over owned values rather than references. I have been
unable to find any cases which _ought_ to be supported and aren't. (Some cases, such as methods
which consume `self`, remain intentionally unsupported.)

And it requires _no downstream code changes_ other than to the `Matcher` implementations themselves.
So existing uses of the assertions don't break and the fundamental design decisions behind the
library remain intact.

## Why not contribute these changes upstream?

I feel that ship has sailed. The changes to GoogleTest were very deep. They touch almost all
assertion code using it. Upstreaming the above changes to restore the original assertion model would
require undoing all of that work on any code depending on GoogleTest. I doubt I would be successful
convincing the folks at Google to pay that price. And I don't want to condition taking advantage of
the improvements Test That! offers on that happening.

## Whither now Test That!?

I really want Rust to have an excellent test assertion library, and so I really want Test That! to
succeed. I'm planning to gather feedback and polish the crate a little longer before releasing
version 1.0. I hope that it will become a go-to choice for testing in the Rust ecosystem.

So please, go forth and try this out. I look forward to your feedback!

[regex]: https://crates.io/crates/regex
[num-traits]: https://crates.io/crates/num-traits
[`verify_that!`]: https://docs.rs/test-that/latest/test_that/macro.verify_that.html
[`matches_pattern!`]: https://docs.rs/test-that/latest/test_that/matchers/macro.matches_pattern.html
[`contains_each!`]: https://docs.rs/test-that/latest/test_that/matchers/containers/macro.contains_each.html
[`is_contained_in!`]: https://docs.rs/test-that/latest/test_that/matchers/containers/macro.is_contained_in.html
[`TestResult`]: https://docs.rs/test-that/latest/test_that/type.TestResult.html
[`contains_exactly!`]: https://docs.rs/test-that/latest/test_that/matchers/containers/macro.contains_exactly.html
[`in_order()`]: https://docs.rs/test-that/latest/test_that/matchers/containers/struct.ContainerContainsUnorderedMatcher.html#method.in_order
[`Matcher`]: https://docs.rs/test-that/latest/test_that/matcher/trait.Matcher.html
[`Describable`]: https://docs.rs/test-that/latest/test_that/matcher/trait.Describable.html
[`result_of!`]: https://docs.rs/test-that/latest/test_that/matchers/macro.result_of.html
[`describe()`]: https://docs.rs/test-that/latest/test_that/matcher/trait.Describable.html#tymethod.describe
[Test That!]: https://crates.io/crates/test-that
[GoogleTest C++ library]: https://github.com/google/googletest
[GoogleTest Rust]: https://crates.io/crates/googletest
[`PartialEq`]: https://doc.rust-lang.org/std/cmp/trait.PartialEq.html
