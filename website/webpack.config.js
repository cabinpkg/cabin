const path = require('path');
const ExtractTextPlugin = require('extract-text-webpack-plugin');
const UglifyJsPlugin = require('uglifyjs-webpack-plugin');
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyPlugin = require('copy-webpack-plugin');
const AutoPrefixer = require('autoprefixer');


const styleCss = new ExtractTextPlugin({
    filename: '../css/style.css'
});
const AutoPrefixerPlugin = AutoPrefixer({
    grid: true
});


module.exports = {
    optimization: {
        minimizer: [
            new UglifyJsPlugin({
                cache: true,
                parallel: true,
                sourceMap: true
            }),
            new OptimizeCSSAssetsPlugin({}),
        ]
    },

    entry: {
        index: ['babel-polyfill', './js/app.js'],
    }, // TOOD: entryとmodule.exportsから出るのを複数に分ければ，cssをapp.jsでimportする必要がなくなる？

    output: {
        path: path.resolve(__dirname, './dist/js'),
        filename: 'app.js',
    },

    plugins: [
        styleCss,
        new CopyPlugin({
            patterns: [
                { from: 'scss/colorize/dark.css', to: '../css/' },
                { from: 'scss/colorize/light.css', to: '../css/' },
                { from: 'assets/', to: '../' },
                { from: 'index.html', to: '../' },
            ],
        }),
    ],

    module: {
        rules: [
            {
                test: /\.js$/,
                exclude: /node_modules/,
                use: {
                    loader: 'babel-loader',
                },
            },
            {
                test: /\.scss$/,
                exclude: [/node_modules/, /colorize/],
                use: styleCss.extract({
                    use: [
                        "css-loader",
                        {
                            loader: "postcss-loader",
                            options: {
                                plugins: [ AutoPrefixerPlugin ],
                            },
                        },
                        "sass-loader",
                    ],
                }),
            },
            {
                test: /\.elm$/,
                exclude: [/elm-stuff/, /node_modules/],
                loader: "elm-webpack-loader",
                options: {
                    optimize: true,
                },
            },
        ],
        noParse: [/\.elm$/],
    },
};
