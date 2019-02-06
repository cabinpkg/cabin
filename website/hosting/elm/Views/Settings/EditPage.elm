module Views.Settings.EditPage exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "content" ]
        [ h2 [] [ text "Edit Page" ]
        , h3 [ style "color" "gray" ]
            [ text "Comming soon..." ]
        ]
