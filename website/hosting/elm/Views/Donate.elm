module Views.Donate exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "donate" ] [
        h2 [ style [ ("color", "red") ] ] [
            text "Sorry...",
            br [] [],
            text "Donate page is not yet implemented.",
            br [] [],
            text "Please wait for it..."
        ]
    ]
