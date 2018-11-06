module Views.Settings.Tokens exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Views.Svgs as Svgs

view : Model -> Html Msg
view model =
    case model.signinUser of
        Success user ->
            div [ class "content" ] [
                h2 [] [ text "Tokens" ],
                p [] [ text """If you want to use the publish command from the command line,
                               you will need to login with `poac login (token)`
                               using one of the tokens listed below."""
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


genTokenList : RemoteData (List Token) -> List (Html Msg)
genTokenList token =
    case token of
        Success uuidList ->
            uuidList
            |> List.map createListItem
        Requesting ->
            [ div [ class "loader" ]
                  [ Svgs.spinner ]
            ]
        _ ->
            [ text "No API key was created so far" ]


createListItem : Token -> Html Msg
createListItem token =
    li [ class "token" ] [
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
