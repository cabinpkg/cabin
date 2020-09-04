module Subscriptions exposing (subscriptions)

import Messages exposing (Msg(..))
import Model exposing (Model)
import Ports exposing (..)
import Browser.Events exposing (onAnimationFrame)


subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.batch
        [ onAnimationFrame GetNewViewport
        , onWidth OnWidthHandle
        ]
