module Subscriptions exposing (subscriptions)

import Messages exposing (Msg(..))
import Model exposing (Model)
import Ports exposing (..)


subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch
        [ onScroll ScrollHandle
        , onWidth OnWidthHandle
        , receivePackages FetchPackages
        , receiveDetailedPackage FetchDetailedPackage
        ]
