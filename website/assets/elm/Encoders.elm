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
      , ( "token", tokenListEncoder user.token )
      , ( "avatar_url", string user.avatar_url )
      , ( "github_link", string user.github_link )
      , ( "published_packages", toListString user.published_packages )
      ]


tokenListEncoder : Maybe (List Token) -> Encode.Value
tokenListEncoder tokenList =
    case tokenList of
        Just tl ->
            list (List.map tokenEncoder tl)
        Nothing ->
            null

tokenEncoder : Token -> Encode.Value
tokenEncoder token =
    Encode.object
      [ ( "id", string token.id )
      , ( "name", string token.name )
      , ( "created_date", string token.created_date )
      , ( "last_used_date", maybe string token.last_used_date )
      ]

toListString : Maybe (List String) -> Encode.Value
toListString mls =
    case mls of
        Just ls ->
            list (List.map string ls)
        Nothing ->
            null
