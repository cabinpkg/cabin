module Views.Settings exposing (view)

import Routing exposing (Route(..))
import Views.NotFound as NotFound
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model id =
    let
        content =
            case id of
                "profile" ->
                    profile model
                "tokens" ->
                    tokens model
                _ ->
                    NotFound.view
    in
        div [ class "settings" ] [
            hr [] [],
            div [ class "container" ] [
                menu id,
                content
            ]
        ]


menu : String -> Html Msg
menu id =
    let
        createMenuItemWithId =
            createMenuItem id
    in
        div [ class "menu" ] [
            nav [] [
                createMenuItemWithId "users/matken11235" "My page",
                createMenuItemWithId "dashboard" "Dashboard",
                createMenuItemWithId "profile" "Edit page",
                createMenuItemWithId "tokens" "Tokens"
            ]
        ]


createMenuItem : String -> String -> String -> Html Msg
createMenuItem id currentId content =
    a [ onClick <| NavigateTo (SettingsRoute currentId)
      , class ("menu-item" ++ (addSelected id currentId))
      ] [ i [ class "fas fa-book-open"
            , style [("font-size", "15px"), ("font-weight", "900")] ] []
        ,text content
    ]


addSelected : String -> String -> String
addSelected id currentId =
    if id == currentId then " selected" else ""


profile : Model -> Html Msg
profile model =
    div [ class "content" ] [
        h2 [ style [ ("color", "red") ] ] [
            text "Sorry...",
            br [] [],
            text "Profile setting is not yet implemented.",
            br [] [],
            text "Please wait for it..."
        ]
    ]


createListItem : Token -> Html Msg
createListItem token =
    li [ class "token" ] [
        i [ class "fas fa-key" ] [],
        a [ class "token-name" ] [ text token.name ],
        button [ class "delete-token", onClick (RevokeToken token.id) ] [
            text "Revoke"
        ],
        div [] [
            a [ class "token-item token-id" ] [ text token.id ],
            a [ class "token-item token-date" ] [
                text ("Created in " ++ (token.created_date)),
                text ", ",
                case token.last_used_date of
                    Nothing ->
                        text "Never used"
                    Just x ->
                        text x
            ]
        ]
    ]


genTokenList : RemoteData (List Token) -> List (Html Msg)
genTokenList token =
    case token of
        Success uuidList ->
            uuidList
            |> List.map createListItem
        Requesting ->
            [ text "Loading..." ]
        _ ->
            [ text "No API key was created so far" ]


tokens : Model -> Html Msg
tokens model =
    case model.loginUser of
        Success user ->
            div [ class "content" ] [
                h2 [] [ text "Tokens" ],
                p [] [ text "If you want to use package commands from the command line, "
                     , text "you will need to login with poac login (token) using one of the tokens listed below."
                ],
                p [] [
                    text "When working in shared environments, ",
                    text "supplying the token on the command line could expose it to prying eyes.",
                    text " To avoid this, enter poac login and supply your token when prompted."
                ],
                input [ placeholder "new token name", onInput HandleTokenInput ] [],
                button [ onClick CreateToken ] [ text "Create a new token" ],
                div [ class "list" ] (genTokenList model.currentToken)
            ]
        _ ->
--            -- TODO: I want to call without clicking
            a [ ] [ text "You are not login" ]
--            a [ onClick <| AutoLogin ] [ text "You are not login" ]
