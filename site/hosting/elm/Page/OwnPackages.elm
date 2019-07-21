module Page.OwnPackages exposing (view)

import Assets
import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Page.NotFound as NotFound
import Route


view : Model -> String -> Html Msg
view model owner =
    main_
        [ class "packages" ]
        (listPackages model owner)


listPackages : Model -> String -> List (Html Msg)
listPackages model owner =
    case model.ownPackages of
        Success packages ->
            div []
                [ text owner ]
                :: List.filterMap buildHtml packages

        Requesting ->
            [ div [ class "spinner" ]
                [ Assets.spinner ]
            ]

        _ ->
            [ NotFound.view ]


buildHtml : RemoteData PackageMetadata -> Maybe (Html Msg)
buildHtml package =
    case package of
        Success p ->
            Just (extractPackage p)
        _ ->
            Nothing


extractPackage : PackageMetadata -> Html Msg
extractPackage package =
    div []
        [ a [ Route.href (Route.Package package.owner package.repo "latest")  ]
            [ text package.repo ]
        ]
