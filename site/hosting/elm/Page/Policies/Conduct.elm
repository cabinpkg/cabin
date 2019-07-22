module Page.Policies.Conduct exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)

view : List (Html Msg)
view =
    [ h2 [] [ text "Code of Conduct" ]
    , h3 [ style "color" "gray" ]
         [ text "Comming soon..." ]
    ]
