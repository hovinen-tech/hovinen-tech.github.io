---
layout: post
title: Clean Code, Horrible Performance. So what?
categories: [opinion]
tags: [software engineering, software development, clean code]
permalink: "/blog/2024/04/08/clean-code-horrible-performance-so-what/"
---
 
Last year, Casey Muratori published a [blog post](https://www.computerenhance.com/p/clean-code-horrible-performance) and YouTube video '"Clean" Code, horrible performance', generating a great deal of discussion. The article and its discussion struck a nerve with me. Reflecting on it, I have refined my own thinking about what Clean Code really means and why it is important.
<!--more-->

I find the the tone and messaging implied in Muratori's article problematic. I have witnessed software developers who stubbornly defend clever but utterly incomprehensible code in the face of complaints from their teammates. They often justify their position with spurious claims about "performance" (often without actually _measuring_ it). But CPU-bound performance is rarely a major business concern, while the ability to understand and reason about the code almost always is. Obsessing over the former while dismissing the latter is a tragic inversion of priority.
## Muratori's position
Muratori writes about how to develop software with an eye towards performance. That's valuable! This article focuses on the blind application of the heuristic "prefer polymorphism to switch statements". Doing so can result in code whose (CPU-bound) performance is about ten times slower than highly optimized code doing the same calculation.

The tone in his article is already clear from its tagline: 'Many programming "best practices" taught today are performance disasters waiting to happen.' The article claims that the value of Clean Code were subjective, implying that it were merely about personal preferences or aesthetics. Performance, on the other hand, is objectively measurable. The implication is that one should ignore Clean Code and focus solely on performance.
## What is Clean Code?
The term _Clean Code_ comes from a 2008 [book](https://www.goodreads.com/book/show/3735293-clean-code) of the same name by [Robert C. Martin](https://en.wikipedia.org/wiki/Robert_C._Martin). The book is full of rules and heuristics about what it means for code to be "clean". Some of these are absolutely right (even brilliant), while others are more questionable. It's certainly not the last word on the subject.

One could define "Clean Code" as being the set of rules and heuristics in Martin's book. I find this definition too reductive. Why then discuss Clean Code at all? Our profession's understanding has come a long way since the book was published.

In recent years, the software engineering world has seen increased focus on the concept of [_cognitive load_](https://en.wikipedia.org/wiki/Cognitive_load). Roughly speaking, this is the amount of working memory the brain needs to learn about a system. *Extraneous cognitive load* -- that which does not depend on the system itself but only on its presentation -- is problematic. Think of overly complex program structure, unclear and inconsistent naming, rules which the code itself does not reveal but which one must "just know". You get the idea.

With that in mind, I propose an improved definition of Clean Code:

> Clean code _minimizes the extraneous cognitive load of persons reading it_.

Henceforth, I'll leave out the word "extraneous" and just refer to cognitive load.

This concept is _objective_ in the sense that cognitive load is a real phenomenon. It has real effects on an engineer's ability to understand, reason about, and maintain code. And this in turn has a real and substantial business impact. It's also _subjective_ in the sense that some idioms will generate more or less cognitive load for different engineers. Those used to object oriented programming have an easier time with polymorphism than those who do mainly functional programming. Functional programmers may be more comfortable with functors and monads than object oriented programmers.

Going back to the book _Clean Code_: some of its heuristics are clearly universal -- using descriptive, consistent naming will reduce cognitive load for any reader. But the book is generally focused on the object-oriented paradigm. Some rules, such as those surrounding the use of inheritance, don't translate easily to other paradigms.
## CPU-bound performance is rarely the right measure
The first problem with this line of thinking is that there is rarely a legitimate business need to improve CPU-bound performance.

Note: _rarely_ does not mean _never_! Yes, there are cases where CPU-bound performance is critical. If there is a _problem_ with CPU-bound performance (or it's clear that there could be one), then there clearly is a business need to fix it. But compared to the amount of code out there, this is rare in practice. In business applications, it's so rare that a professional software engineer in that space can easily go through their entire _career_ without having to think about CPU-bound performance _even once_.

> _Side note:_ _I/O-bound performance_, such as database performance, _does_ often come up in the context of business applications. Muratori does not discuss these issues in his blog post, so I do not discuss those further here.

Let's illustrate this with an example. Before trying to optimize code for performance, one should always ask the question of _return on investment_. Suppose you find a way to invest one hour of your time as a software engineer earning US$100,000 per year to save 250 clock cycles every time a bit of backend code is executed. (This is much more than the savings Muratori claims from his example.) We'll put aside the question of cognitive load and just ask under what conditions this would have a positive return on investment. We'll conservatively estimate that your work on this optimization costs your employer US$100, based on compensation and other employment costs.

How could this investment generate a return? Well, assuming no business folks or end users are complaining, about the only way is in compute resources. Using the [AWS EC2](https://aws.amazon.com/ec2/pricing/on-demand/) prices as a guide, we'll estimate (conservatively) that a single CPU core costs US$0.10 per hour. So you'd have to save between 1000 *hours* of compute resources to make up that investment. Assuming the code runs on a 2.5 Ghz core, those 250 saved cycles could execute ca. 10 million times per second, or 36 billion times per hour. So the code you have optimized must run 36 *trillion* times to make up that investment. If this code runs once per request on a service (as most business application code does) serving 1000 QPS, your one hour investment would not pay off in less than 1000 years.

_No business would agree to an investment with such a low return._

These are, of course, rough calculations which don't take into account all sorts of other variables. But even a two order of magnitude improvement in the return on investment would not be enough to justify it.

As I've said, if there's a _problem_ with performance, or it's clear that there will be one, then there's a business need to fix it. And that means identifying the problem and its root cause and introducing the necessary optimizations. Absent that, I would want to see some argument for a positive ROI before agreeing to invest in improving CPU-bound performance.
## Reducing cognitive load
Okay, what about cognitive load? What about the return of investment on making code cleaner? The costs and benefits are of a different nature. We are comparing the time spent cleaning the code to the time saved _by other engineers_ understanding and reasoning about it. Investing ten minutes (effectively!) improving the code quality will typically save more than ten minutes understanding and reasoning about it _every time it is read_. After all, the person writing the code already has the context to understand how to change it easily. And code will likely be read more often that it is written.

So one rarely needs to worry about ROI when reducing cognitive load. It's almost always a slam dunk case.

There are only a few cases where I might push back if a team member wants to invest in improving code quality:
 * The code in question is about to be deleted (a prototype, for example). Then I'd ask whether it's really worth cleaning things up given that it presumably won't be read much any more.
 * The team member is planning a really large investment, such as several days or weeks of work. Then I'd want to see a clear plan of action and I'd consider the investment's priority compared to other projects.

## Straw men
Aside from the fundamental issue above with his argument, Muratori attacks some [straw men](https://en.wikipedia.org/wiki/Straw_man):
* He claims that Clean Coders say one should "never use an if or switch statement, but always use polymorphism". Now, there is a _heuristic_ in _Clean Code_ which says that one should _prefer_ polymorphism through inheritance to switch statements. But Muratori's paraphrasing ignores context and is at best a gross exaggeration.

  In fact, it appears that Muratori's objections are mostly about the use of vtable-based dynamic dispatch in languages like C++. It's unfortunate then that the tone of his article is so broad. He casts the entirety of Clean Code in doubt based just on that one objection.

* He claims that the example he uses -- calculating areas of geometric shapes were representative because it is "Clean Coders' own example". Folks, the example of calculating areas is a _toy example for learning about polymorphism through inheritance_.

  Would one write the code like that in a real application? In a vacuum, I doubt that using polymorphism to calculate shape areas reduces cognitive load at all compared to using a switch statement or some other optimization. Cases where polymorphism through inheritance makes sense are typically more complex and at a much higher level of abstraction. CPU-bound performance is seldom a real issue in such cases.

## Conclusion
It's not clear whether Muratori intended to argue against Clean Code in general or just to warn against the overzealous application of a few specific rules. Regardless, I hold that his article's whole thesis is flawed. There's no real conflict between Clean Code and performance. Legitimate business needs -- including performance -- clearly take precedence over concerns about the cognitive load the code creates. If a legitimate business need for CPU-bound performance comes into conflict with some Clean Code heuristic, then it's fine to ignore that heuristic. But the bottom line is that investing in reducing extraneous cognitive load always always pays off, while investing in CPU-bound performance only does so in rare cases. If one disagrees with what measures really help with cognitive load, then let's have that discussion. But this discussion of performance is a distraction.

I know that "Clean Code" can feel subjective, even dogmatic. How do you know whether one way or the other reduces cognitive load? Ultimately, it comes down to what is easiest for your team. If your colleagues are telling you that your code is hard to understand and maintain, listen to them!

> P.S. There is a [debate](https://github.com/cmuratori/misc/blob/main/cleancodeqa.md) between Muratori and Robert C. Martin in response to Muratori's article. It goes into much greater detail about the background of Muratori's views. He seems to have been motivated by a perceived lack of concern about performance by modern developers. And his objections to Clean Code appear to revolve around vtable-based dynamic dispatch.
> 
> My take: blaming Clean Code, or even dynamic dispatch, for performance issues isn't helpful. Clean Code and performance need not be conflicting goals. And dynamic dispatch is rarely a cause of performance issues. To the extent that software performance is increasingly a problem, it's because our _expectations_ have grown so much. The software must support every kind of device and platform. It must be accessible and secure and privacy-protecting. It must render beautifully on 4k displays. It must support [emojis](https://github.com/cmuratori/misc/blame/41127fc9c9a459b8a5bb9acf52dcc365ab7b9afa/cleancodeqa.md#L137). And it has to delivered quickly. All of these things necessitate more and more abstract frameworks and runtimes, increase complexity, and ultimately slow systems down. One can argue that software has become too bloated, but that's hardly because it were "too clean".
>
> Muratori is singularly focused on CPU-bound performance. His article suggests that he can only really argue in those terms. He clearly does not see the value in vtable-based dynamic dispatch. He could have just written an article arguing, as he does in the debate, that _dynamic dispatch with vtables_ holds no value and should not be used. (I would take exception to that point as well, but that's another discussion.) Instead, he ranted about Clean Code in general, as though it all revolved around that one technique. Such myopia and reductionism does not serve our profession well.
