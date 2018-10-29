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
        maybe_content =
            case id of
--                "dashboard" ->
--                    Just (dashboard model)
--                "profile" ->
--                    Just (profile model)
                "tokens" ->
                    Just (tokens model)
                _ ->
                    Nothing
    in
        case maybe_content of
            Just content ->
                div [ class "settings" ]
                    [ menu id
                    , content
                    ]
            Nothing ->
                NotFound.view



menu : String -> Html Msg
menu id =
    div [ class "menu" ] [
        nav []
        [ a [ onClick <| NavigateTo (UsersRoute "matken11235")
            , class "menu-item"
            ]
            [ i [ class "fas fa-book-open"
                , style
                  [ ("font-size", "15px")
                  , ("font-weight", "900")
                  , ("margin-left", "20px")
                  ]
                ] []
            , a [ class "menu-name" ] [ text "My Page" ]
            ]
--        , a [ onClick <| NavigateTo (SettingsRoute "dashboard")
--              , class ("menu-item" ++ (addSelected id "dashboard"))
--              ] [ i [ class "fas fa-bolt"
--                    , style
--                      [ ("font-size", "15px")
--                      , ("font-weight", "900")
--                      , ("margin-left", "20px")
--                      , ("padding-left", "4px")
--                      , ("padding-right", "4px")
--                      ]
--                    ] []
--                , a [ class "menu-name" ] [ text "Dashboard" ]
--            ]
--        , a [ onClick <| NavigateTo (SettingsRoute "profile")
--              , class ("menu-item" ++ (addSelected id "profile"))
--              ] [ i [ class "fas fa-pencil-alt"
--                    , style
--                      [ ("font-size", "15px")
--                      , ("font-weight", "900")
--                      , ("margin-left", "20px")
--                      , ("padding-left", "1px")
--                      , ("padding-right", "1px")
--                      ]
--                    ] []
--                , a [ class "menu-name" ] [ text "Edit Page" ]
--            ]
        , a [ onClick <| NavigateTo (SettingsRoute "tokens")
              , class ("menu-item" ++ (addSelected id "tokens"))
              ] [ i [ class "fas fa-cog"
                    , style
                      [ ("font-size", "15px")
                      , ("font-weight", "900")
                      , ("margin-left", "20px")
                      , ("padding-left", "1px")
                      , ("padding-right", "1px")
                      ]
                    ] []
                , a [ class "menu-name" ] [ text "Tokens" ]
            ]
        ]
    ]


dashboard : Model -> Html Msg
dashboard model =
    div [ class "content" ]
    [ h2 [] [ text "Dashboard" ]
    , div [ class "dashboard-storage-content" ]
      [ i [ class "fas fa-hdd"
            , style
              [ ("font-size", "20px")
              , ("font-weight", "900")
              , ("margin-left", "20px")
              ]
            ] []
        , span [ class "dashboard-storage" ] [ text "Storage (MB)" ]
        , div [ class "ct-chart" ] []
      ]
    ]


createMenuItem : String -> String -> String -> Html Msg
createMenuItem id currentId content =
    a [ onClick <| NavigateTo (SettingsRoute currentId)
      , class ("menu-item" ++ (addSelected id currentId))
      ] [ i [ class "fas fa-book-open"
            , style [("font-size", "15px"), ("font-weight", "900")] ] []
        , text content
    ]


addSelected : String -> String -> String
addSelected id currentId =
    if id == currentId then " selected" else ""


profile : Model -> Html Msg
profile model =
    div [ class "content" ]
      [ h2 [] [ text "Edit Page" ]
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
    case model.signinUser of
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
            div [ class "content" ]
              [ h2 [] [ text "Tokens" ]
              , a [] [ text "Please signin" ]
              ]
