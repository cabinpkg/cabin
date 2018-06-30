module Decoders exposing (..)

import Json.Decode as JD exposing (..)
import Json.Decode.Extra exposing ((|:))
import Model exposing (..)


userDecoder : JD.Decoder User
userDecoder =
    succeed User
      |: (field "id" string)
      |: (field "name" string)
      |: (field "avatar" string)
      |: (field "apikey" (nullable string))
      |: (field "github" string)
      |: (field "published_packages" (nullable (list string)))
