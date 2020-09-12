module Page.Home exposing (view)

import Css exposing (..)
import Css.Media exposing (withMediaQuery)
import Css.Global as Global exposing (children, everything)
import GlobalCss exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (class, css, src, alt, id, type_, placeholder, name, autocomplete, for, href)
import Html.Styled.Events exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Model exposing (..)


homeViewWidth : Model -> Style
homeViewWidth model =
    if model.width > 1000 then
        property "width" "calc(85% - 2 * (100vw - 1000px) / 200)"
    else if model.width > 1200 then
        width (pct 80)
    else
        width (vw 90)


view : Model -> Html Msg
view model =
    main_
        [ css [ withMediaQuery
                  [ "(max-width: 1150px)" ]
                  [ children [
                      everything [
                          width (pct 90) |> important ]
                      ]
                  ]
              , homeViewWidth model
              , marginRight auto
              , marginLeft auto
              , textAlign center
              ]
        ]
        [ img [ css [ property "pointer-events" "none"
                    , width (vw 60)
                    ]
              , src "/images/terminal.svg"
              , alt "terminal demonstration"
              ] []
        , phraseView
        , getStartedView model.isFadein
        , section model.isFadein
        , recognizableLinkGlobalStyle
        ]


headlineTextStyle : Style
headlineTextStyle =
    Css.batch
        [ marginTop (px 15)
        , marginBottom (px 15)
        , fontSize (rem 0.8)
        , lineHeight (num 1.5)
        , fontWeight normal
        ]


phraseView : Html Msg
phraseView =
    div
        [ css
            [ width (px 800)
            , marginTop (px 50)
            , marginRight auto
            , marginLeft auto
            , fontFamilies ["montserrat", .value sansSerif]
            , fontStyle normal
            ]
        ]
        [ h1 []
            [ text "Package Manager for C++ Developers"
            ]
        , h2 [ css [ headlineTextStyle ] ]
            [ text "Poac is a C++ package manager which is for open source software."
            ]
        , h3 [ css [ headlineTextStyle ] ]
            [ text """Easy to introduce to your projects;
                   you can use packages intuitively."""
            ]
        , searchBox
        ]


algoliaSearchInputStyle : Style
algoliaSearchInputStyle =
    Css.batch
        [ width (px 300)
        , height (px 40)
        , marginTop (px 20)
        , padding (px 12)
        , border3 (px 2) solid (hex "e4e4e4")
        , borderRadius (px 4)
        , fontFamilies [ "montserrat", .value sansSerif ]
        , fontWeight (int 600)
        , fontSize (px 11)
        , fontStyle normal
        , legacyTransition ".2s"
        , legacyBoxShadow "0 3px 15px 0 #b9b9b9"
        , legacyBoxSizing "border-box"
        , appearance "none"
        , outline zero
        , focus
            [ outline zero
            , borderColor (hex "3a96cf")
            , legacyBoxShadow "0 3px 15px 0 rgba(58, 150, 207, 0.1)"
            ]
        ]


algoliaGlobalStyle : Html Msg
algoliaGlobalStyle =
    let
        bgColor =
            backgroundColor (rgba 241 241 241 0.35)
        emStyle =
            Global.descendants
                [ Global.typeSelector "em"
                    [ fontWeight (int 700)
                    , fontStyle normal
                    , backgroundColor (rgba 58 150 207 0.1)
                    , padding4 (px 2) (px 0) (px 2) (px 2)
                    ]
                ]
   in
    Global.global
        [ Global.class "aa-hint"
            [ color (hex "c3c3c3") ]
        , Global.class "aa-dropdown-menu"
            [ border3 (px 2) solid (rgba 228 228 228 0.6)
            , borderTopWidth (px 1)
            , borderRadius (px 4)
            , fontFamilies [ "Montserrat", .value sansSerif ]
            , fontSize (px 11)
            , width (px 300)
            , legacyBoxShadow "4px 4px 0 rgba(241, 241, 241, 0.35)"
            , legacyBoxSizing "border-box"
            ]
        , Global.class "aa-suggestion"
            [ padding (px 12)
            , borderTop3 (px 1) solid (rgba 228 228 228 0.6)
            , cursor pointer
            , legacyTransition ".2s"
            , legacyDisplayFlex
            , legacyJustifyContentSpaceBetween
            , legacyAlignItems "center"
            , hover [ bgColor ]
            , Global.withClass "aa-cursor" [ bgColor ]
            , Global.children
                [ Global.span
                    [ firstChild [ emStyle ]
                    , lastChild [ emStyle ]
                    ]
                ]
            ]
        ]


searchBox : Html Msg
searchBox =
    div [ id "aa-input-container"
        , css
            [ children
                [ everything [ unselectable ]
                ]
            ]
        ]
        [ Html.Styled.form []
            [ input
                [ css [ algoliaSearchInputStyle ]
                , type_ "search"
                , id "aa-search-input"
                , placeholder "Search packages"
                , name "search"
                , autocomplete False
                , onKeyDown Search
                , onInput OnSearchInput
                ]
                []
            , label
                [ for "aa-search-input"
                , css
                    [ visibility hidden
                    , display block
                    ]
                ]
                [ text "Search packages" ]
            ]
        , algoliaGlobalStyle
        ]


onKeyDown : (Int -> msg) -> Attribute msg
onKeyDown tagger =
    on "keydown" (Json.map tagger keyCode)


fadeinStyle : Style
fadeinStyle =
    Css.batch
        [ opacity (int 0)
        , legacyTransform "translate(0, 50px)"
        , legacyTransition "all 300ms"
        ]


scrollinStyle : Style
scrollinStyle =
    Css.batch
        [ opacity (int 1)
        , legacyTransform "translate(0, 0)"
        ]


addFadeinStyle : Bool -> Style
addFadeinStyle bool =
    if bool then Css.batch [ fadeinStyle, scrollinStyle ] else fadeinStyle


getStartedStyle : Style
getStartedStyle =
    Css.batch
        [ marginTop (px 80)
        , paddingTop (px 40)
        , textAlign left
        , fontFamilies [ "montserrat", .value sansSerif ]
        , fontWeight (int 300)
        ]


pGetStartedStyled : List (Attribute msg) -> List (Html msg) -> Html msg
pGetStartedStyled =
    styled p
        [ width (px 450)
        ]


getStartedView : IsFadein -> Html Msg
getStartedView isFadein =
    div
        [ css
            [ getStartedStyle
            , addFadeinStyle isFadein.getStart
            ]
        ]
        [ h1 [] [ text "Getting started" ]
        , pGetStartedStyled
            []
            [ text """Want to use poac right now?
                    Let's get started in just a sec!
                    Unless, if you use a peculiar an operating system,
                     you can install Poac easily by the following command.
                    """
            ]
        , pGetStartedStyled
            [ css
                [ before
                    [ property "content" "$ "
                    ]
                , fontFamilies [ "Menlo", .value sansSerif ]
                , fontWeight (int 500)
                , padding (px 20)
                , marginBottom (px 30)
                , legacyBoxShadow "0 3px 15px 0 #b9b9b9"
                ]
            ]
            [ text "curl -fsSL https://sh.poac.pm | bash"
            ]
        , pGetStartedStyled
            [ class "details" ]
            [ text "Please refer to "
            , a
                [ class "book-link"
                , href "https://docs.poac.io"
                ]
                [ text "The Poac Book" ]
            , text " for details."
            ]
        ]


cardStyle : String -> Style
cardStyle delay =
    let
        paddingSize = px 50
    in
    Css.batch
        [ width (pct 24)
        , display inlineBlock
        , padding paddingSize
        , verticalAlign top
        , textAlign center
        , fontFamilies [ "montserrat", .value sansSerif ]
        , hover
            [ legacyTransitionDelay delay
            , legacyBoxShadow "0 3px 15px 0 #b9b9b9"
            ]
        , firstChild
            [ property "margin-left" "calc(-1 * #{$padding-size})"
            ]
        , lastChild
            [ property "margin-right" "calc(-1 * #{$padding-size})"
            ]
        ]


h2CardStyled : List (Attribute msg) -> List (Html msg) -> Html msg
h2CardStyled =
    styled h2
        [ marginTop (px 0)
        , marginBottom (px 30)
        , fontStyle normal
        ]


pCardStyled : List (Attribute msg) -> List (Html msg) -> Html msg
pCardStyled =
    styled p
        [ marginTop (px 0)
        , marginBottom (px 0)
        ]


sectionUsefulInterface : IsFadein -> Html Msg
sectionUsefulInterface isFadein =
    div
        [ css
            [ cardStyle "0s"
            , addFadeinStyle isFadein.section1
            ]
        ]
        [ h2CardStyled
            []
            [ text "Useful Interface" ]
        , pCardStyled
            []
            [ text """Poac is a C++ package manager and
                    a CLI application provided for a client."""
            ]
        , pCardStyled
            []
            [ text """Poac is easy to use because it refers to
                    the other package managers' modern interface."""
            ]
        ]


sectionAccelerateDevelopment : IsFadein -> Html Msg
sectionAccelerateDevelopment isFadein =
    div
        [ css
            [ cardStyle ".1s"
            , addFadeinStyle isFadein.section1
            ]
        ]
        [ h2CardStyled
            []
            [ text "Accelerate Development" ]
        , pCardStyled
            []
            [ text """Even if you have managed packages manually or
                    if you have used other package managers,
                    you can quickly introduce Poac."""
            ]
        , pCardStyled
            []
            [ text "Poac flexibly copes with small-scale and large-scale development."
            ]
        ]


sectionOpenSourceSoftware : IsFadein -> Html Msg
sectionOpenSourceSoftware isFadein =
    div
        [ css
            [ cardStyle ".2s"
            , addFadeinStyle isFadein.section1
            ]
        ]
        [ h2CardStyled
            []
            [ text "Open Source Software" ]
        , pCardStyled
            []
            [ text "All related to poac is open source."
            ]
        , pCardStyled
            []
            [ text """You can contribute to Poac by publishing packages,
                    or you can contribute to Poac directly."""
            ]
        , pCardStyled
            []
            [ text "The client-side is written in C++, and it will be self-hosted."
            ]
        ]


section : IsFadein -> Html Msg
section isFadein =
    div
        [ class "section"
        , css
            [ paddingTop (px 80)
            , legacyDisplayFlex
            , legacyJustifyContentSpaceAround
            ]
        ]
        [ sectionUsefulInterface isFadein
        , sectionAccelerateDevelopment isFadein
        , sectionOpenSourceSoftware isFadein
        ]
