module ContactList.View exposing (indexView)

import Common.View exposing (warningMessage)
import Contact.View exposing (contactView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Html.Keyed exposing (..)
import Messages exposing (..)
import Model exposing (..)


indexView : Model -> Html Msg
indexView model =
    div
        [ id "home_index" ]
        (viewContent model)


viewContent : Model -> List (Html Msg)
viewContent model =
    case model.contactList of
        NotRequested ->
            [ text "" ]

        Requesting ->
            [ searchSection model
            , warningMessage
                "fa fa-spin fa-cog fa-2x fa-fw"
                "Searching for contacts"
                (text "")
            ]

        Failure error ->
            [ warningMessage
                "fa fa-meh-o fa-stack-2x"
                error
                (text "")
            ]

        Success page ->
            [ searchSection model
            , paginationList page
            , div
                []
                [ contactsList model page ]
            , paginationList page
            ]


searchSection : Model -> Html Msg
searchSection model =
    div
        [ class "filter-wrapper" ]
        [ div
            [ class "overview-wrapper" ]
            [ h3
                []
                [ text <| headerText model ]
            ]
        , div
            [ class "form-wrapper" ]
            [ Html.form
                [ onSubmit HandleFormSubmit ]
                [ resetButton model "reset"
                , input
                    [ type_ "search"
                    , placeholder "Search contacts..."
                    , value model.search
                    , onInput HandleSearchInput
                    ]
                    []
                ]
            ]
        ]


headerText : Model -> String
headerText model =
    case model.contactList of
        Success page ->
            let
                totalEntries =
                    page.total_entries

                contactWord =
                    if totalEntries == 1 then
                        "contact"
                    else
                        "contacts"
            in
                if totalEntries == 0 then
                    ""
                else
                    (toString totalEntries) ++ " " ++ contactWord ++ " found"

        _ ->
            ""


contactsList : Model -> ContactList -> Html Msg
contactsList model page =
    if page.total_entries > 0 then
        page.entries
            |> List.map contactView
            |> Html.Keyed.node "div" [ class "cards-wrapper" ]
    else
        warningMessage
            "fa fa-meh-o fa-stack-2x"
            "No contacts found..."
            (resetButton model "btn")


paginationList : ContactList -> Html Msg
paginationList page =
    List.range 1 page.total_pages
        |> List.map (paginationLink page.page_number)
        |> Html.Keyed.ul [ class "pagination" ]


paginationLink : Int -> Int -> ( String, Html Msg )
paginationLink currentPage page =
    let
        classes =
            classList [ ( "active", currentPage == page ) ]
    in
        ( toString page
        , li
            []
            [ a
                [ classes
                , onClick <| Paginate page
                ]
                []
            ]
        )


resetButton : Model -> String -> Html Msg
resetButton model className =
    let
        hide =
            model.search == ""

        classes =
            classList
                [ ( className, True )
                , ( "hidden", hide )
                ]
    in
        a
            [ classes
            , onClick ResetSearch
            ]
            [ text "Reset search" ]
