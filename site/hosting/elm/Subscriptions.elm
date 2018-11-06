module Subscriptions exposing (subscriptions)

import Model exposing (Model)
import Messages exposing (Msg(..))
import Ports exposing (..)

subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch
        [ scroll ScrollHandle
        , receiveSigninUser Signin
        , receiveSigninId FetchSigninId
        , receiveUser FetchUser
        , receiveToken FetchToken
        , receivePackages FetchPackages
        , receiveDetailedPackage FetchDetailedPackage
        ]
