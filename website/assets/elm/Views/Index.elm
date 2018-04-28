module Views.Index exposing (indexView)

import Routing exposing (Route(HomeIndexRoute))
import Views.Common exposing (warningMessage)
import Views.Contact exposing (contactView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


indexView : Model -> Html Msg
indexView model =
    div [ class "index" ]
        [ headerView, topView ]


headerView : Html Msg
headerView =
    header [ class "header" ]
        [ nav []
            [ ul [ class "nav nav-pills pull-right" ]
                [
                li []
                    [ a [href "/packages"] [text "Packages"] ],
                li []
                    [ a [href "/docs"] [text "Documentation"] ],
                li []
                    [ a [href "/signin"] [text "Signin"],
                      text " or ",
                      a [href "/signup"] [text "Signup"] ]
                ]
            ],
            a [ onClick <| NavigateTo HomeIndexRoute, style [ ("cursor", "pointer") ] ]
                [ img [
                    alt "logo",
                    src "/images/icon&logo.png",
                    style [ ("width", "20%"), ("height", "20%") ]
                ] [] ]
        ]

topView : Html Msg
topView =
    div []
        [ h1 []
            [ text "Modern Package Manager for C++ Developers" ],
          h2 []
            [ text "poacpm is the package manager that for open source." ],
          h2 []
            [ text "Easy to introduce to your C++ project, you can use the package intuitively." ]
         ]


--indexView : Model -> Html Msg
--indexView model =
--    div [ id "home_index" ]
--        (viewContent model)
--
--
--viewContent : Model -> List (Html Msg)
--viewContent model =
--    case model.contactList of
--        NotRequested ->
--            [ text "" ]
--
--        Requesting ->
--            [ searchSection model
--            , warningMessage
--                "fa fa-spin fa-cog fa-2x fa-fw"
--                "Searching for contacts"
--                (text "")
--            ]
--
--        Failure error ->
--            [ warningMessage
--                "fa fa-meh-o fa-stack-2x"
--                error
--                (text "")
--            ]
--
--        Success page ->
--            [ searchSection model
--            , paginationList page
--            , div
--                []
--                [ contactsList model page ]
--            , paginationList page
--            ]
--
--
--searchSection : Model -> Html Msg
--searchSection model =
--    div
--        [ class "filter-wrapper" ]
--        [ div
--            [ class "overview-wrapper" ]
--            [ h3
--                []
--                [ text <| headerText model ]
--            ]
--        , div
--            [ class "form-wrapper" ]
--            [ Html.form
--                [ onSubmit HandleFormSubmit ]
--                [ resetButton model "reset"
--                , input
--                    [ type_ "search"
--                    , placeholder "Search contacts..."
--                    , value model.search
--                    , onInput HandleSearchInput
--                    ]
--                    []
--                ]
--            ]
--        ]
--
--
--headerText : Model -> String
--headerText model =
--    case model.contactList of
--        Success page ->
--            let
--                totalEntries =
--                    page.total_entries
--
--                contactWord =
--                    if totalEntries == 1 then
--                        "contact"
--                    else
--                        "contacts"
--            in
--                if totalEntries == 0 then
--                    ""
--                else
--                    (toString totalEntries) ++ " " ++ contactWord ++ " found"
--
--        _ ->
--            ""
--
--
--contactsList : Model -> ContactList -> Html Msg
--contactsList model page =
--    if page.total_entries > 0 then
--        page.entries
--            |> List.map contactView
--            |> Html.Keyed.node "div" [ class "cards-wrapper" ]
--    else
--        warningMessage
--            "fa fa-meh-o fa-stack-2x"
--            "No contacts found..."
--            (resetButton model "btn")
--
--
--paginationList : ContactList -> Html Msg
--paginationList page =
--    List.range 1 page.total_pages
--        |> List.map (paginationLink page.page_number)
--        |> Html.Keyed.ul [ class "pagination" ]
--
--
--paginationLink : Int -> Int -> ( String, Html Msg )
--paginationLink currentPage page =
--    let
--        classes =
--            classList [ ( "active", currentPage == page ) ]
--    in
--        ( toString page
--        , li
--            []
--            [ a
--                [ classes
--                , onClick <| Paginate page
--                ]
--                []
--            ]
--        )
--
--
--resetButton : Model -> String -> Html Msg
--resetButton model className =
--    let
--        hide =
--            model.search == ""
--
--        classes =
--            classList
--                [ ( className, True )
--                , ( "hidden", hide )
--                ]
--    in
--        a
--            [ classes
--            , onClick ResetSearch
--            ]
--            [ text "Reset search" ]
