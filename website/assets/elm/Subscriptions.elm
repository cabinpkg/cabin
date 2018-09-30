module Subscriptions exposing (subscriptions)

import Model exposing (Model)
import Messages exposing (Msg(..))
import Ports exposing (..)

subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch
        [ getAuth Login
        , recieveUser FetchUser
        , recieveToken FetchToken
        , recievePackages FetchPackages
        , recieveDetailedPackage FetchDetailedPackage
        ]
