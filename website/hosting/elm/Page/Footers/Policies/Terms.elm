module Page.Footers.Policies.Terms exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)

view : List (Html Msg)
view =
    [ h2 [] [ text "Terms of Service" ]
    , h3 [ style "color" "gray" ]
         [ text "Comming soon..." ]
    ]
