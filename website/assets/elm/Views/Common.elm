module Views.Common exposing (..)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Routing exposing (Route)
import Messages exposing (Msg(..))
import Routing exposing (Route(..))


warningMessage : String -> String -> Html Msg -> Html Msg
warningMessage iconClasses message content =
    div [ class "warning" ]
        [ i [ class iconClasses ] []
        , h4 []
            [ text message ]
        , content
        ]

aLink : String -> Html Msg
aLink name =
    a [ href ("/" ++ (String.toLower name)) ]
        [text name]

aNavLink : Route -> String -> Html Msg
aNavLink route name =
    a [ onClick <| NavigateTo route, style [ ("cursor", "pointer") ] ]
        [ text name ]


backToHomeLink : Html Msg
backToHomeLink =
    a [ onClick <| NavigateTo HomeIndexRoute, style [ ("cursor", "pointer") ] ]
        [ text "← Back to index" ]
