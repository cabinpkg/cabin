module Encoders exposing (..)

import Json.Encode as Encode exposing (..)
import Json.Encode.Extra exposing (maybe)
import Maybe
import List
import Model exposing (..)


userEncoder : User -> Encode.Value
userEncoder user =
    Encode.object
      [ ( "id", string user.id )
      , ( "name", string user.name )
      , ( "token", toListString user.token )
      , ( "avatar_url", string user.avatar_url )
      , ( "github_link", string user.github_link )
      , ( "published_packages", toListString user.published_packages )
      ]

toListString : Maybe (List String) -> Encode.Value
toListString mls =
    case mls of
        Just ls ->
            list (List.map string ls)
        Nothing ->
            null
