module Subscriptions exposing (subscriptions)

import Messages exposing (Msg(..))
import Model exposing (Model)
import Ports exposing (..)


subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch
        [ onScroll ScrollHandle
        , onWidth OnWidthHandle
        , receiveSigninUser Signin
        , receiveSigninId FetchSigninId
        , receiveUser FetchUser
        , receiveToken FetchToken
        , receivePackages FetchPackages
        , receiveDetailedPackage FetchDetailedPackage
        ]
