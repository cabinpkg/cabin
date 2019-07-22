module Page.PackageVersions exposing (view)

import Assets
import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Page.NotFound as NotFound
import Route


view : Model -> String -> String -> Html Msg
view model owner repo =
    main_
        [ class "packages" ]
        (listPackages model owner repo)


listPackages : Model -> String -> String -> List (Html Msg)
listPackages model owner repo =
    case model.packageVersions of
        Success versions ->
            let
                bh = buildHtml owner repo
            in
            div []
                [ text <| owner ++ "/" ++ repo ]
                :: List.map bh versions

        Requesting ->
            [ div [ class "spinner" ]
                [ Assets.spinner ]
            ]

        _ ->
            [ NotFound.view ]


buildHtml : String -> String -> String -> Html Msg
buildHtml owner repo version =
    div []
       [ a [ Route.href (Route.Package owner repo version)  ]
           [ text version ]
       ]
