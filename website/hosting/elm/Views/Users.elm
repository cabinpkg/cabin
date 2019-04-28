module Views.Users exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Views.NotFound as NotFound
import Views.Svgs as Svgs


view : Model -> String -> Html Msg
view model userId = -- TODO: unused variable
    case model.otherUser of
        Success user ->
            main_ [ class "users" ]
                [ div [ class "user" ]
                    [ info user
                    , package model.listPackages
                    ]
                ]

        Requesting ->
            main_ [ class "users" ]
                [ div [ class "user" ]
                    [ div [ class "spinner" ]
                        [ Svgs.spinner ]
                    ]
                ]

        _ ->
            NotFound.view


info : User -> Html Msg
info user =
    div [ class "info" ]
        [ img [ class "avatar-top", alt user.id, src user.photo_url, width 200, height 200 ] []
        , h2 [ class "user-id" ] [ text user.id ]
        , text user.name
        , hr [ class "divider" ] []
        , a [ class "link", href user.github_link ]
            [ i [ class "fab fa-github" ] []
            , text user.id
            ]
        ]


package : RemoteData (List Package) -> Html Msg
package ownedPackages =
    case ownedPackages of
        Success listPackages ->
            div [ class "package" ]
                (h3 [ class "first-header" ] [ text "Owned packages" ]
                    :: List.map packageView listPackages
                )

        Requesting ->
            div [ class "spinner" ]
                [ Svgs.spinner ]

        _ ->
            div [ class "spinner" ]
                [ Svgs.spinner ]


packageView : Package -> Html Msg
packageView pack =
    div [ class "list-item" ]
        [ a
            [ class "packname"
            , href ("/packages/" ++ pack.name)
            ]
            [ text pack.name ]
        , span [ class "version" ] [ text pack.version ]
        , p [ class "description" ] [ text pack.description ]
        ]
