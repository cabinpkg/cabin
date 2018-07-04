module Decoders exposing (..)

import Json.Decode as JD exposing (..)
import Json.Decode.Extra exposing ((|:))
import Model exposing (..)


userDecoder : JD.Decoder User
userDecoder =
    succeed User
      |: (field "id" string)
      |: (field "name" string)
      |: (field "token" (nullable (list string)))
      |: (field "avatar_url" string)
      |: (field "github_link" string)
      |: (field "published_packages" (nullable (list string)))
