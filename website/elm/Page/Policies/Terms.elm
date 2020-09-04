module Page.Policies.Terms exposing (view)

import Css exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css)
import Messages exposing (..)

view : List (Html Msg)
view =
    [ h2 [] [ text "Terms of Service" ]
    , h3 [ css [ property "color" "gray" ] ]
         [ text "Comming soon..." ]
    ]
