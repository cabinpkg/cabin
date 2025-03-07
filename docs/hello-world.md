---
title: Hello, World!
---

# Hello, World!

You can get started with just a few commands as the demo shows. Let's create a new Cabin project:

```console
you:~$ cabin new hello_world
     Created binary (application) `hello_world` package
```

Now, you can use the `cabin run` command to run your application:

```console
you:~$ cd hello_world
you:~/hello_world$ cabin run
 Compiling src/main.cc
   Linking hello_world
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45386s
   Running cabin-out/dev/hello_world
Hello, world!
```
