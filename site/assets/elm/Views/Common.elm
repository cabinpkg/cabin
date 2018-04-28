module Views.Common exposing (..)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (Msg(..))
import Routing exposing (Route(..))


warningMessage : String -> String -> Html Msg -> Html Msg
warningMessage iconClasses message content =
    div
        [ class "warning" ]
        [ span
            [ class "fa-stack" ]
            [ i [ class iconClasses ] [] ]
        , h4
            []
            [ text message ]
        , content
        ]


backToHomeLink : Html Msg
backToHomeLink =
    a
        [ onClick <| NavigateTo HomeIndexRoute, style [ ("cursor", "pointer") ] ]
        [ text "← Back to index" ]
