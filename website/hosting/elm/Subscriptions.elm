module Subscriptions exposing (subscriptions)

import Messages exposing (Msg(..))
import Model exposing (Model)
import Ports exposing (..)


subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.batch
        [ onScroll ScrollHandle
        , onWidth OnWidthHandle
        , receiveOwnPackages FetchOwnPackages
        , receiveVersions FetchPackageVersions
        , receivePackage FetchPackage
        ]
