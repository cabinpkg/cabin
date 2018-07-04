module Views.Settings exposing (view)

import Routing exposing (Route(..))
import Views.Header as Header
import Views.NotFound as NotFound
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import List


view : Model -> String -> Html Msg
view model id =
    let
        content =
            case id of
                "profile" ->
                    profile model
                "keys" ->
                    keys model
                _ ->
                    NotFound.view model
    in
        div [ class "settings" ] [
            Header.view model,
            div [ class "container" ] [
                menu id,
                content
            ]
        ]

menu : String -> Html Msg
menu id =
    div [ class "menu" ] [
        nav [] [
            h3 [ class "menu-heading" ] [ text "Personal settings" ],
            createMenuItem id "profile" "profile",
            createMenuItem id "keys" "API keys"
        ]
    ]

createMenuItem : String -> String -> String -> Html Msg
createMenuItem id currentId content =
    a [ onClick <| NavigateTo (SettingsRoute currentId)
      , class ("menu-item" ++ (addSelected id currentId))
      ] [
        text content
    ]

addSelected : String -> String -> String
addSelected id currentId =
    if id == currentId then " selected" else ""

profile : Model -> Html Msg
profile model =
    div [ class "content" ] [
        text "profile settings"
    ]

createListItem : String -> Html Msg
createListItem str =
    li [ class "list-item" ] [
        i [ class "fas fa-key" ] [],
        text str
    ]

keys : Model -> Html Msg
keys model =
    case model.loginUser of
        Success user ->
            let
                uuidText =
                    case user.token of
                        Nothing ->
                            [ text "No API key was created so far" ]
                        Just uuidList ->
                            uuidList
                                |> List.map createListItem
            in
                div [ class "content" ] [
                    h2 [] [ text "API keys" ],
                    p [] [
                        text "If you want to use package commands from the command line, ",
                        text "you will need to login with poac login (token) using one of the tokens listed below."
                    ],
                    p [] [
                        text "When working in shared environments, ",
                        text "supplying the token on the command line could expose it to prying eyes.",
                        text " To avoid this, enter poac login and supply your token when prompted."
                    ],
                    button [ onClick NewUuid ] [ text "Generate a new API key" ],
                    div [ class "list" ] uuidText
                ]
        _ ->
            -- TODO: I want to call without clicking
            a [ onClick <| AutoLogin ] []
