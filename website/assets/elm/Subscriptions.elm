module Subscriptions exposing (subscriptions)

import Model exposing (Model)
import Messages exposing (Msg(SelectMeta))
import Ports exposing (selectMeta)

subscriptions : Model -> Sub Msg
subscriptions model =
    Sub.batch [ selectMeta SelectMeta ]
