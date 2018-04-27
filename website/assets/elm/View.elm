module View exposing (..)

import Common.View exposing (warningMessage, backToHomeLink)
import Contact.View exposing (showContactView)
import ContactList.View exposing (indexView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick)
import Messages exposing (..)
import Model exposing (..)
import Routing exposing (Route(..))


view : Model -> Html Msg
view model =
    section
        []
        [ headerView
        , div []
            [ page model ]
        ]


headerView : Html Msg
headerView =
    header
        [ class "main-header" ]
        [ h1
            []
            [ text "Phoenix and Elm: A real use case" ]
        ]


page : Model -> Html Msg
page model =
    case model.route of
        HomeIndexRoute ->
            indexView model

        ShowContactRoute id ->
            showContactView model

        NotFoundRoute ->
            notFoundView


notFoundView : Html Msg
notFoundView =
    warningMessage
        "fa fa-meh-o fa-stack-2x"
        "Page not found"
        backToHomeLink
