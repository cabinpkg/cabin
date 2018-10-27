module Json.Encode.Extra exposing (dict, maybe)

{-| Convenience functions for turning Elm values into Json values.

@docs dict, maybe

-}

import Dict exposing (Dict)
import Json.Encode exposing (Value, encode, int, null, object)


{-| Encode a Maybe value. If the value is `Nothing` it will be encoded as `null`

    import Json.Encode exposing (..)


    maybe int (Just 50)
    --> int 50


    maybe int Nothing
    --> null

-}
maybe : (a -> Value) -> Maybe a -> Value
maybe encoder =
    Maybe.map encoder >> Maybe.withDefault null


{-| Turn a `Dict` into a JSON object.

    import Json.Encode exposing (..)
    import Dict


    Dict.fromList [ ( "Sue", 38 ), ( "Tom", 42 ) ]
        |> dict identity int
        |> encode 0
    --> """{"Sue":38,"Tom":42}"""

-}
dict : (comparable -> String) -> (v -> Value) -> Dict comparable v -> Value
dict toKey toValue dict =
    Dict.toList dict
        |> List.map (\( key, value ) -> ( toKey key, toValue value ))
        |> object
