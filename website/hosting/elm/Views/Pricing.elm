module Views.Pricing exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Html Msg
view =
    main_ [ class "pricing" ]
        [ h2 [ style "color" "gray" ]
            [ text "Comming soon..." ]
        ]
