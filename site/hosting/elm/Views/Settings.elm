module Views.Settings exposing (view)

import Views.Settings.EditPage as EditPage
import Views.Settings.Account as Account
import Views.Settings.Dashboard as Dashboard
import Views.Settings.Packages as Packages
import Views.Settings.Tokens as Tokens
import Views.NotFound as NotFound

import Routing exposing (Route(..))
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import String.Extra exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model current_route =
    let
        maybe_content =
            case current_route of
                "edit_page" ->
                    Just (EditPage.view model)

                "account" ->
                    Just (Account.view model)

                "dashboard" ->
                    Just (Dashboard.view model)

                "packages" ->
                    Just (Packages.view model)

                "tokens" ->
                    Just (Tokens.view model)

                _ ->
                    Nothing
    in
        case maybe_content of
            Just content ->
                div [ class "settings" ]
                    [ menu current_route model.signinId
                    , content
                    ]

            Nothing ->
                NotFound.view



type alias MenuItem =
    { route : String
    , current_route : String
    , icon : String
    }


menu : String -> String -> Html Msg
menu current_route signinId =
    let
        menuItems =
            [ MenuItem "edit_page" current_route "fa-pencil-alt"
            , MenuItem "account" current_route "fa-user"
            , MenuItem "dashboard" current_route "fa-bolt"
            , MenuItem "packages" current_route "fa-cube"
            , MenuItem "tokens" current_route "fa-key"
            ]
    in
        div [ class "menu" ]
            [ nav []
                  <| a [ onClick <| NavigateTo (UsersRoute signinId)
                       , class "menu-item"
                       ]
                       [ i [ class "fas fa-book-open" ] []
                       , a [ class "menu-name" ] [ text "My Page" ]
                       ]
                     :: List.map attachMenuItem menuItems
            ]


attachMenuItem : MenuItem -> Html Msg
attachMenuItem menuItem =
    a [ onClick <| NavigateTo (SettingsRoute menuItem.route)
      , class ("menu-item" ++ (addSelected menuItem.current_route menuItem.route))
      ]
      [ i [ class <| "fas " ++ menuItem.icon ] []
      , a [ class "menu-name" ]
          [ text <| humanize menuItem.route ]
      ]


addSelected : String -> String -> String
addSelected id currentId =
    if id == currentId then " selected" else ""
