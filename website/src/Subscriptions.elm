module Subscriptions exposing (subscriptions)

import Messages exposing (Msg(..))
import Model exposing (Model)
import Browser.Events exposing (onAnimationFrame, onResize)


subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.batch
        [ onAnimationFrame OnAnimationFrame
        , onResize (\width _ -> GotNewWidth width)
        ]
