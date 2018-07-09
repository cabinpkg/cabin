module Decoders exposing (..)

import Json.Decode as Decode exposing (..)
import Json.Decode.Extra exposing ((|:))
import Model exposing (..)


userDecoder : Decode.Decoder User
userDecoder =
    succeed User
        |: (field "id" string)
        |: (field "name" string)
        |: (field "token" (nullable tokenListDecoder))
        |: (field "avatar_url" string)
        |: (field "github_link" string)
        |: (field "published_packages" (nullable (list string)))


tokenListDecoder : Decode.Decoder (List Token)
tokenListDecoder =
    list tokenDecoder

tokenDecoder : Decode.Decoder Token
tokenDecoder =
    succeed Token
        |: (field "id" string)
        |: (field "name" string)
        |: (field "created_date" string)
        |: (field "last_used_date" (nullable string))
