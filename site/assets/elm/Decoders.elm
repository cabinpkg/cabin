module Decoders exposing (..)

import Json.Decode as JD exposing (..)
import Json.Decode.Extra exposing ((|:))
import Model exposing (..)


userInfoDecoder : JD.Decoder UserInfo
userInfoDecoder =
    succeed UserInfo
      |: (field "name" string)
      |: (field "avatar_url" string)
