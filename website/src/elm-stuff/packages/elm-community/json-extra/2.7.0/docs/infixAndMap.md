## Json.Decode.Extra.(|:)


Infix version of `andMap` that makes for a nice DSL when decoding objects.

Consider the following type alias for a `Location`:

```elm
type alias Location =
    { id : Int
    , name : String
    , address : String
    }
```

We can use `(|:)` to build up a decoder for `Location`:

```elm
locationDecoder : Decoder Location
locationDecoder =
    succeed Location
        |: (field "id" int)
        |: (field "name" string)
        |: (field "address" string)
```



If you're curious, here's how this works behind the scenes, read on.

`Location` is a type alias, and type aliases give you a convenience function
that returns an instance of the record in question. Try this out in `elm-repl`:

```elm
> type alias Location = { id : Int, name: String, address: String }

> Location
<function> : Int -> String -> String -> Repl.Location

> Location 1 "The White House" "1600 Pennsylvania Ave"
{ id = 1, name = "The White House", address = "1600 Pennsylvania Ave" }
```

In other words, if you call the `Location` function, passing three arguments,
it will return a new `Location` record by filling in each of its fields. (The
argument order is based on the order in which we listed the fields in the
type alias; the first argument sets `id`, the second argument sets `name`, etc.)

Now try running this through `elm-repl`:

```elm
> import Json.Decode exposing (succeed, int, string, field)

> succeed Location
<function>
    : Json.Decode.Decoder
        (Int -> String -> String -> Repl.Location)
```

So `succeed Location` gives us a `Decoder (Int -> String -> String -> Location)`.
That's not what we want! What we want is a `Decoder Location`. All we have so
far is a `Decoder` that wraps not a `Location`, but rather a function that
returns a `Location`.

What `|: (field "id" int)` does is to take that wrapped function and pass an
argument to it.

```elm
> import Json.Decode exposing (succeed, int, string, field)

> (field "id" int)
<function> : Json.Decode.Decoder Int

> succeed Location |: (field "id" int)
<function>
    : Json.Decode.Decoder
        (String -> String -> Repl.Location)
```

Notice how the wrapped function no longer takes an `Int` as its first argument.
That's because `(|:)` went ahead and supplied one: the `Int` wrapped by the decoder
`(field "id" int)` (which returns a `Decoder Int`).

Compare:

```elm
> succeed Location
Decoder (Int -> String -> String -> Location)

> succeed Location |: (field "id" int)
Decoder (String -> String -> Location)
```

We still want a `Decoder Location` and we still don't have it yet. Our decoder
still wraps a function instead of a plain `Location`. However, that function is
now smaller by one argument!

Let's repeat this pattern to provide the first `String` argument next.

```elm
> succeed Location
Decoder (Int -> String -> String -> Location)

> succeed Location |: (field "id" int)
Decoder (String -> String -> Location)

> succeed Location |: (field "id" int) |: (field "name" string)
Decoder (String -> Location)
```

Smaller and smaller! Now we're down from `(Int -> String -> String -> Location)`
to `(String -> Location)`. What happens if we repeat the pattern one more time?

```elm
> succeed Location
Decoder (Int -> String -> String -> Location)

> succeed Location |: (field "id" int)
Decoder (String -> String -> Location)

> succeed Location |: (field "id" int) |: (field "name" string)
Decoder (String -> Location)

> succeed Location |: (field "id" int) |: (field "name" string) |: (field "address" string)
Decoder Location
```

Having now supplied all three arguments to the wrapped function, it has ceased
to be a function. It's now just a plain old `Location`, like we wanted all along.

We win!
