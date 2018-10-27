# URL Parser

This library helps you turn URLs into nicely structured data.

It is designed to be used with `elm-lang/navigation` to help folks create single-page applications (SPAs) where you manage browser navigation yourself.

> **Note:** This library is meant to serve as a baseline for future URL parsers. For example, it does not handle query parameters and hashes right now. It is more to (1) get folks started using URL parsers and (2) help us gather data on exactly which scenarios people face.


## Examples

Here is a simplified REPL session showing a parser in action:

```elm
> import UrlParser exposing ((</>), s, int, string, parseHash)

> parseHash (s "blog" </> int) { ... , hash = "#blog/42" }
Just 42

> parseHash (s "blog" </> int) { ... , hash = "#/blog/13" }
Just 13

> parseHash (s "blog" </> int) { ... , hash = "#/blog/hello" }
Nothing

> parseHash (s "search" </> string) { ... , hash = "#search/dogs" }
Just "dogs"

> parseHash (s "search" </> string) { ... , hash = "#/search/13" }
Just "13"

> parseHash (s "search" </> string) { ... , hash = "#/search" }
Nothing
```

Normally you have to put many of these parsers to handle all possible pages though! The following parser works on URLs like `/blog/42` and `/search/badger`:

```elm
import UrlParser exposing (Parser, (</>), s, int, string, map, oneOf, parseHash)

type Route = Blog Int | Search String

route : Parser (Route -> a) a
route =
  oneOf
    [ map Blog (s "blog" </> int)
    , map Search (s "search" </> string)
    ]

-- parseHash route { ... , hash = "#/blog/58" }    == Just (Blog 58)
-- parseHash route { ... , hash = "#/search/cat" } == Just (Search "cat")
-- parseHash route { ... , hash = "#/search/31" }  == Just (Search "31")
-- parseHash route { ... , hash = "#/blog/cat" }   == Nothing
-- parseHash route { ... , hash = "#/blog" }       == Nothing
```

Notice that we are turning URLs into nice [union types](https://guide.elm-lang.org/types/union_types.html), so we can use `case` expressions to work with them in a nice way.

Check out the `examples/` directory of this repo to see this in use with `elm-lang/navigation`.


## Background

I first saw this general idea in Chris Done&rsquo;s [formatting][] library. Based on that, Noah and I outlined the API you see in this library. Noah then found Rudi Grinberg&rsquo;s [post][] about type safe routing in OCaml. It was exactly what we were going for. We had even used the names `s` and `(</>)` in our draft API! In the end, we ended up using the &ldquo;final encoding&rdquo; of the EDSL that had been left as an exercise for the reader. Very fun to work through!

[formatting]: http://chrisdone.com/posts/formatting
[post]: http://rgrinberg.com/posts/primitive-type-safe-routing/
