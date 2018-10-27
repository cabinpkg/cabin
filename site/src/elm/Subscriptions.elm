module Subscriptions exposing (subscriptions)

import Model exposing (Model)
import Messages exposing (Msg(..))
import Ports exposing (..)

subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch
        [ scroll ScrollHandle
        , getAuth Signin
        , recieveUser FetchUser
        , recieveToken FetchToken
        , recievePackages FetchPackages
        , recieveDetailedPackage FetchDetailedPackage
        ]
