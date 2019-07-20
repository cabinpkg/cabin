module Page.Footers.Policies.Privacy exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)

view : List (Html Msg)
view =
    [ h2 [] [ text "Privacy Policy" ]
    , h3 [ style "color" "gray" ]
         [ text "Comming soon..." ]
    ]
