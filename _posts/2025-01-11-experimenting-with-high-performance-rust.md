---
layout: post
title: "Experimenting with high performance Rust"
categories: [walkthrough]
tags: [rust, performance]
permalink: "/blog/2025/01/11/experimenting-with-high-performance-rust/"
has_mermaid: false
---

A few weeks ago I attended a "hacking session" organized by the Rust Munich meetup. The participants
competed on a single task: given one billion temperature measurements from about 10000 different
weather stations, output aggregate statistics for each station. The goal was to write a program
which reads the input from a file and produces the output as quickly as possible. This problem is
fascinating: quite simple yet illustrating a lot of ideas about performance optimization. In this
blog post, I trace the journey from a naive solution to a highly performant one, reducing the
wall clock runtime by 97%.

<!--more-->

## The input and output formats

The input is a CSV file where each line contains the station name and a measurement separated by a
semicolon. The station name is a UTF-8 encoded string, normally the name of its city. It may contain
punctuation (though never a semicolon). It is never quoted. It can be at most the shorter of 100
bytes or 50 characters.

The measurement is a decimal number, strictly between -100 and 100, with one decimal place which
is always included. When the measurement has absolute value less than 1, the leading zero is always
present.

The examples on which performance is measured have one billion measurements in total. This is large
enough that one can't really expect to hold all of it in memory at once.

The output has the following format:

```
{
    Station name 1=min/avg/max,
    Station name 2=min/avg/max,
    Station name 3=min/avg/max,
    ...
}
```

where:

- There is exactly one entry per station,
- `min/avg/max` are decimal numbers with the same rules as with the input, and
- The station names are alphabetically sorted.

## Our starting point

We start with a straightforward solution:

```rust
{% raw %}
use std::{
    collections::{hash_map::Entry, HashMap},
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let result = process_file(&Path::new(&args.get(1).expect("Require a filename")));
    output(std::io::stdout(), &result);
}

fn process_file(path: &Path) -> Vec<(String, f64, f64, f64)> {
    let file = File::open(path).expect("Cannot open file");
    let reader = BufReader::new(file);
    let mut cities = HashMap::<String, _>::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                panic!("Error parsing line {line_number}: {err}")
            }
        };
        let mut line_split = line.split(";");
        let city = line_split.next().expect("City should be present");
        let measurement = line_split
            .next()
            .expect("Measurement should be present")
            .parse::<f64>()
            .expect("Valid measurement");
        match cities.entry(city.to_string()) {
            Entry::Occupied(mut occupied_entry) => {
                let (min, max, sum, count) = occupied_entry.get_mut();
                *min = f64::min(*min, measurement);
                *max = f64::max(*max, measurement);
                *sum += measurement;
                *count += 1;
            }
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert((measurement, measurement, measurement, 1));
            }
        }
    }
    let mut results = cities
        .into_iter()
        .map(|(city, (min, max, sum, count))| (city.to_string(), min, sum / count as f64, max))
        .collect::<Vec<_>>();
    results.sort_by(|(v1, _, _, _), (v2, _, _, _)| v1.cmp(v2));
    results
}

fn output(mut writer: impl Write, lines: &[(String, f64, f64, f64)]) {
    writeln!(writer, "{{").unwrap();
    for (ref city, min, mean, max) in lines[0..lines.len() - 1].iter() {
        writeln!(writer, "    {city}={min:0.1}/{mean:0.1}/{max:0.1},").unwrap();
    }
    let (ref city, min, mean, max) = lines[lines.len() - 1];
    writeln!(writer, "    {city}={min:0.1}/{mean:0.1}/{max:0.1}").unwrap();
    write!(writer, "}}").unwrap();
}
{% endraw %}
```

When writing this solution, I also wrote a series of automated tests, including the following cases:

- The statistics output for a single positive measurement are correct.
- The statistics output for a single negative measurement are correct.
- The statistics output for a single station with two measurements are correct.
- The statistics output for two different stations with one measurement each are correct.
- The stations in the output are sorted alphabetically.
- The output for a sample file with 100 entries matches the official results.

Later on, I added another test to cover more possible edge cases: the output for a sample file with
1 million entries matches the official results

Now that we are confident of the correctness of the solution, let's see how it performs on some
sample input. To measure performance, we use the tool
[hyperfine](https://github.com/sharkdp/hyperfine). It runs the program ten times and calculates
statistics on all the runs. I ran all performance tests on a 12-core AMD Ryzen 5 2600X with 32GiB of
memory.

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     198.745 s ±  5.985 s    [User: 195.841 s, System: 1.856 s]
  Range (min … max):   193.537 s … 209.930 s    10 runs
```

We see that the initial solution processes a file of 1 billion entries in just under 200 seconds.

## Optimizing our solution

At first glance, there are many potential opportunities to optimize the initial solution, including:

- Better exploiting library functionality to load the file more efficiently,
- Reducing allocations of `String`,
- Making the search for the column separator more efficient,
- Parallelizing processing,
- Loading the file asynchronously and potentially out of order.

Which ones will bring the most benefit? To answer that, we'll use the tool
[Flamegraph](https://github.com/flamegraph-rs/flamegraph), which gives us a nice visualization of
where the CPU time is really going.

![Flamegraph of our initial solution](/assets/2025/01/flamegraph-initial.svg)

Here and in the flamegraphs below we only show the most relevant rows and skip the lower frames.

### Removing the `String` allocation

Our first observation is that we do a lot of copying and allocation which doesn't seem necessary.
Whenever we read a line, we copy the station name out of it into a new `String` to invoke
`HashMap::entry`. Let's try working directly with the string slice instead:

```rust
if let Some((min, max, sum, count)) = cities.get_mut(city) {
    *min = f64::min(*min, measurement);
    *max = f64::max(*max, measurement);
    *sum += measurement;
    *count += 1;
} else {
    cities.insert(city.to_string(), (measurement, measurement, measurement, 1));
}
```

Let's now run our benchmark again with this change:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
 Time (mean ± σ):     175.358 s ±  0.936 s    [User: 172.667 s, System: 1.856 s]
  Range (min … max):   174.403 s … 177.421 s    10 runs
```

The total runtime dropped to 175 seconds, about a 12% improvement. We're on our way!

### Reading byte arrays rather than strings

Let's take another look at the flamegraph after making the previous optimization:

![Flamegraph after reducing String allocation](/assets/2025/01/flamegraph-after-reducing-string-allocation.svg)

We notice a lot of time spent in `BufRead::read_line`. Could we reduce that overhead by reading the
bytes from the file into a byte array? We'll try the following:

```rust
let mut line_buffer = Vec::with_capacity(80);
while let Ok(count) = reader.read_until('\n' as u8, &mut line_buffer) {
    if count == 0 { break; }
    let len = if line_buffer[count - 1] == '\n' as u8 { count - 1 } else { count };
    let Some((separator_index, _)) = 
        line_buffer.iter().enumerate().find(|(_, c)| **c == ';' as u8)
    else {
        panic!("Invalid line");
    };
    let city = unsafe { std::str::from_utf8_unchecked(&line_buffer[..separator_index]) };
    let measurement_str =
        unsafe { std::str::from_utf8_unchecked(&line_buffer[separator_index + 1..len]) };
    let Ok(measurement) = measurement_str.parse::<f64>() else {
        panic!("Could not parse {:?}", measurement_str.as_bytes());
    };
    ...
}
```

When parsing the measurement, we have to be careful to get a string slice which corresponds
_exactly_ to the measurement value. The trailing newline must not be present, since that would lead
to a parse failure. So we have some extra logic to cut off the newline character if present.

We also apply another optimization: since we "know" that the input is already UTF-8, we skip the
check that it is valid and reinterpret the byte array directly as a string slice. This reduces the
overhead in our case, but must never be used with input one does not trust.

Running our benchmark again:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     77.963 s ±  0.319 s    [User: 75.728 s, System: 1.844 s]
  Range (min … max):   77.437 s … 78.331 s    10 runs
```

This reduces the total runtime to about 78 seconds -- a 55% reduction in runtime compared to the
previous version and a 61% reduction compared to the original version!

### Improving the hash algorithm

We also see on the flamegraph a fair amount of time spent in the `HashMap`. Could we improve on
that? The hash function used by default in the standard library is
[hashbrown](https://github.com/rust-lang/hashbrown), which is quite performant. But perhaps we can
get some better performance using the [ahash](https://crates.io/crates/ahash) crate instead.

Dropping `ahash` in and rerunning the benchmark, we get the following:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     71.378 s ±  3.088 s    [User: 69.203 s, System: 1.815 s]
  Range (min … max):   69.687 s … 80.045 s    10 runs
```

This reduces runtime to about 71 seconds -- another 8% performance gain.

### Optimizing the search for the field separator

Another area where we spend some time is in searching for the column separator in each line. The
code searches byte by byte from the beginning of the line. But we already know a lot about where the
separator will be relative to the _end_ of the line. Each temperature measurement is at least three
one-byte characters, and may have up to two more characters. The separator is immediately in front
of that. So, instead of searching from the beginning of the line, we can search backwards from the
_end_ of the line, skipping three bytes. The separator will then be within the first three bytes
encountered.

So we change:

```rust
let Some((separator_index, _)) = 
    line_buffer.iter().enumerate().find(|(_, c)| **c == ';' as u8)
```

into this:

```rust
let Some((separator_index, _)) =
    line_buffer.iter().enumerate().rev().skip(3).find(|(_, c)| **c == ';' as u8)
```

Rerunning the benchmark:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     58.738 s ±  0.325 s    [User: 56.629 s, System: 1.809 s]
  Range (min … max):   58.295 s … 59.237 s    10 runs
```

This reduces runtime to about 59 seconds, an 18% improvement.

### Reading larger buffers

Let's take another look at the flamegraph:

![Flamegraph after optimizing the search for the field separator](/assets/2025/01/flamegraph-field-separator-search-opt.svg)

There is still a lot of time spent in `std::io::read_until`. Could we improve on that by reading
larger blocks and finding the line breaks ourselves?

This is where things become complicated. Up until now, we relied on the standard library to find the
line breaks while reading the file. If we read a larger fixed-size block, then it is unlikely that
the block will end exactly on a line break. So we have the following rules for each block we read:

- If it is not the last block of the file, do not process the part from the last newline character
  to the end of the block. Instead, save a copy of that part for the next iteration.
- If it is not the first block read, concatenate the saved part from the previous read with the
  portion up until the first newline character, then process that as a line.
- Continue processing after the first newline character as usual.

Implementing this logic and running our benchmark, we see the following:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     45.488 s ±  0.907 s    [User: 43.266 s, System: 1.990 s]
  Range (min … max):   44.114 s … 47.258 s    10 runs
```

The total runtime has fallen to just over 45 seconds -- a 22% improvement over the previous
iteration.

Let's take a look at the flamegraph to see our current state:

![Flamegraph when reading data in larger blocks](/assets/2025/01/flamegraph-read-larger-blocks.svg)

We see that most of the runtime is going to working with the hash table and parsing the number now.
Our opportunities for improving (serial) performance are becoming thinner.

## Parallelizing the solution

We see in the flamegraph a fair amount of CPU-bound computation, mostly working with the hash map
and parsing measurements. Can we distribute that work effectively to all of our cores?

We start from the block-based approach of the previous solution. Rather than process each block
serially after it is read, we dispatch its processing to a task queue. The tasks are processed from
a managed thread pool. For this purpose we introduce [tokio](https://tokio.rs/).

Remember that the whole file can't fit into memory. If we just keep loading blocks and dispatching
tasks, then we'll probably run out. To avoid that, we allocate a fixed set of buffers and load data
until the set of buffers is exhausted. When a task finishes its work, it returns its buffer to the
pool where it can be reused to load more data.

To return buffers to the pool, we use an
[mpsc channel](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html).

To allow all the threads in our pool to add their measurements to the aggregated data, our hash map
needs to be concurrent. We switch from `AHashMap` to the concurrent hash map
[`DashMap`](https://crates.io/crates/dashmap).

Having implemented our parallel solution, let's check its performance:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):     29.172 s ±  0.306 s    [User: 284.694 s, System: 30.867 s]
  Range (min … max):   28.670 s … 29.514 s    10 runs
```

This is a bit disappointing.

Since the solution runs on multiple cores in parallel, we now distinguish between _wall clock time_
and _total runtime_. The wall clock time of just over 29 seconds constitutes a 36% improvement over
the best serial implementation. That's an improvement, but consider that the work is now spread
across 12 cores. The total runtime of almost 285 seconds is the worst yet. So while we gained
something, we should try to do better.

Let's look at the flamegraph:

![Flamegraph for the initial parallel solution](/assets/2025/01/flamegraph-parallel-solution.svg)

The firs things which sticks out is how much time we spend in `dashmap::lock::RawRwLock`. That
suggests that the problem is lock contention.

### Reducing lock contention

Let's modify our scheme. Rather than work on the same `DashMap`, we'll create an `AHashMap` for each
buffer. A task processing a block updates the data in the hash map associated to the buffer it
holds. Since only one task can hold a buffer at a time, there is no problem with concurrent access
and no need to use a concurrent hash map like `DashMap`.

When all tasks are done, we go through the buffers and fold all the hash maps into a single one,
from which we produce the output. I decided to use [rayon](https://crates.io/crates/rayon) to
parallelize this, but in truth it's such a tiny amount of work that it makes virtually no difference
whether one parallelizes it or not.

Implementing this and checking the performance, we see the following:

```
➜ hyperfine --warmup 1 'cargo run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):      6.687 s ±  0.061 s    [User: 75.252 s, System: 2.301 s]
  Range (min … max):    6.571 s …  6.763 s    10 runs
```

Much better! The wall clock time is just under 6.7 seconds -- a 77% improvement over the initial
parallel implementation and an 85% improvement over the best serial implementation. The total runtime
of just over 75 seconds is still significantly more than that of the best serial solution, but not so
much as to nullify the benefits one gets from parallelization.

Le's take another look at the flamegraph:

![Flamegraph after using separate HashMap's](/assets/2025/01/flamegraph-separate-hashmaps.svg)

At this point, the time is again dominated by working with the hash maps and parsing the measurement
-- just as the best serial solution. Is there much left to improve?

### Using tokio-uring

One factor which we haven't considered yet is whether the work is sufficiently CPU-bound. We are
reading the file one block at a time and then dispatching tasks. Does the program end up waiting
on the I/O for part of the time rather than processing data?

We could try is to read the file blocks asynchronously and concurrently. This may improve the
dispatch of tasks in some cases. Rather than read one block at a time linearly through the file, we
dispatch a bunch of calls to read blocks asynchronously and start processing whichever data we can
as soon as we can. In cases where the data might be read out of order, this could allow work to be
dispatched to the CPUs more quickly.

For this, we introduce the [`tokio-uring`](https://crates.io/crates/tokio-uring), which is a wrapper
around the [IoUring](https://en.wikipedia.org/wiki/Io_uring) syscall on Linux. It makes all major
filesystem I/O operations asynchronous and integrates them with the Tokio runtime. This makes it
quite performant and ergonomic to integrate asynchronous I/O into a Tokio-based program.

We modify our program to load the file asynchronously. The program now spawns a new asynchronous
task each time it loads a block into a buffer. As a result, it now has more housekeeping. The
processing task needs to have the first few characters of the following block. So it cannot be
dispatched until the next block has also been read.

When a block is loaded, it is passed via an mpsc channel to a task which handles the spawning of
processing jobs. This task keeps track of which processing jobs can be dispatched based on the rules
described above.

There is also one caveat: the `File` struct in `tokio-uring` is not `Send`, so it cannot participate
in an async task which may cross thread boundaries. To mitigate this, we run the entire file read
operation on a single thread, dispatching the jobs on a
[`LocalSet`](https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html). This is not a problem in
practice: that task only orchestrates the reading of blocks and is therefore not CPU-bound.

Let's check the performance of this solution:

```
➜ hyperfine --warmup 1 'cargo +nightly run --release -- ../../samples/weather_1B.csv'
Benchmark 1: cargo +nightly run --release -- ../../samples/weather_1B.csv
  Time (mean ± σ):      6.425 s ±  0.061 s    [User: 70.860 s, System: 3.061 s]
  Range (min … max):    6.299 s …  6.498 s    10 runs
```

6.4 seconds wall clock time, 71 seconds total runtime. A 4% improvement over the previous solution.
It's a little better, but the difference is pretty minimal. Most likely IoUring would bring more
benefits in different circumstances -- reading simultaneously from multiple files, perhaps -- but in
this case, the CPU-bound runtime dominates.

Taking another look at the flamegraph:

![Flamegraph for the iouring-based solution](/assets/2025/01/flamegraph-iouring.svg)

We see not much difference from the previous flamegraph. We may be closing in on the limit of what
is possible.

## Some more things I tried but didn't actually bring improvements

In attempting to further optimize performance, I tried a couple more things which _didn't_ actually
improve performance.

### Using fixed byte vectors for the city hash map key

The use of `String` as a key for the `HashMap` storing station data implies a bit of overhead. The
`String` allocates heap memory for the station name and then stores a pointer to the allocated
memory. Could we improve performance further by using a fixed-size byte array as key?

The answer is no. Using `String` also carries an advantage: if a city name has only four one-byte
characters, only those four bytes participate in hashing and comparing equality. A fixed-size array
does not have this advantage, and performance suffers much more than any gains from reducing memory
indirection.

### Using SIMD to find the line and column separators

Right now we are searching for newlines and column separators character by character. Could we
benefit by using [SIMD](https://doc.rust-lang.org/std/simd/index.html) instructions to compare up to
16 characters at once?

In theory, this may be possible. Indeed, a careful examination of the flamegraph shows that the
`AHashMap` implementation already makes use of SIMD. But my attempts to use it actually slowed the
program down slightly. Most likely this is due to alignment. To get good performance out of SIMD
instructions, the data blocks should be aligned to 16 byte boundaries. Lines of the file have
arbitrary alignment. In any case, the flamegraph tells us that searching for the newline isn't such
a huge contributor to runtime.

## Conclusion

In the end, we were able to reduce the runtime of almost 200 seconds to about 6.4 seconds -- a 97%
improvement. Not bad!

The total CPU time was 71 seconds for the best parallel solution and 43 seconds for the best
non-parallel solution. It feels as though there is still room for improvement on the parallel
solution to bring the total CPU time closer to that of the non-parallel solution. But it's not clear
what more one can do.

Find my solution on [GitHub](https://github.com/hovinen/hack-evening-2024-4) under the
[`solutions/hovinen`](https://github.com/hovinen/hack-evening-2024-4/tree/main/solutions/hovinen)
directory. If you find more opportunities to improve its performance,
[let me know!](http://localhost:4000/index.html#contact).

The sample files were generated with the
[`data-generator`](https://github.com/hovinen/hack-evening-2024-4/tree/main/data-generator) tool
located in the same repository.
