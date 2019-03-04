module Views.Pricing exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "pricing" ]
        [ h2 [ style "color" "gray" ]
            [ text "Comming soon..." ]
        ]
