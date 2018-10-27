## Json.Decode.Extra.andMap

Imagine you have a data type for a user

```elm
import Date (Date)

type alias User =
  { id                : Int
  , createdAt         : Date
  , updatedAt         : Date
  , deletedAt         : Maybe Date
  , username          : Maybe String
  , email             : Maybe String
  , isAdmin       : Bool
  }
```

You can use `andMap` to incrementally apply decoders to your `User` type alias
by using that type alias as a function. Recall that record type aliases are
also functions which accept arguments in the order their fields are declared. In
this case, `User` looks like

```elm
User : Int -> Date -> Date -> Maybe Date -> Maybe String -> Maybe String -> Bool -> User
```

And also recall that Elm functions can be partially applied. We can use these
properties to apply each field of our JSON object to each field in our user one
field at a time. All we need to do is also wrap `User` in a decoder and step
through using `andMap`.

```elm
userDecoder : Decoder User
userDecoder =
    succeed User
        |> andMap (field "id" int)
        |> andMap (field "createdAt" date)
        |> andMap (field "updatedAt" date)
        |> andMap (field "deletedAt" (maybe date))
        |> andMap (field "username" (maybe string))
        |> andMap (field "email" (maybe string))
        |> andMap (field "isAdmin" bool)
```

This is a shortened form of

```elm
userDecoder : Decoder User
userDecoder =
    succeed User
        |> andThen (\f -> map f (field "id" int))
        |> andThen (\f -> map f (field "createdAt" date))
        |> andThen (\f -> map f (field "updatedAt" date))
        |> andThen (\f -> map f (field "deletedAt" (maybe date)))
        |> andThen (\f -> map f (field "username" (maybe string)))
        |> andThen (\f -> map f (field "email" (maybe string)))
        |> andThen (\f -> map f (field "isAdmin" bool))
```

See also: The [docs for `(|:)`](https://github.com/elm-community/json-extra/blob/master/docs/infixAndMap.md)
